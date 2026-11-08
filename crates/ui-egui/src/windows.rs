//! Secondary floating windows (Window menu).

use crate::SoundApp;
use crate::theme::{Tokens, mono, rgb};
use egui::{Color32, Sense, vec2};
use serde_json::json;
use soundcraft_model::{AutomationMode, ChannelFormat, ClipContent};
use soundcraft_playback::AudioConfig;
use soundcraft_playback::devices::{AudioSystem, DeviceInfo};
use std::sync::Arc;

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    audio_health(app, ctx);
    automation(app, ctx);
    color_palette(app, ctx);
    disk_usage(app, ctx);
    system_usage(app, ctx);
    task_manager(app, ctx);
    metadata(app, ctx);
    event_list(app, ctx);
    midi_keyboard(app, ctx);
    workspace(app, ctx);
    configurations(app, ctx);
    playback_engine(app, ctx);
    io_setup(app, ctx);
    crate::midi_devices::show(app, ctx);
    #[cfg(not(target_arch = "wasm32"))]
    crate::net_ui::show(app, ctx);
    shortcuts_window(app, ctx);
    clip_effects(app, ctx);
}

fn audio_health(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_audio_health;
    win(ctx, &mut open, "Session Audio Health", vec2(620.0, 360.0), |ui| {
        let refresh = ui.button("Refresh Report").clicked();
        if app.audio_health_report.is_none() || refresh {
            app.audio_health_report = Some(app.engine.execute("session.audio_health", &json!({})).map_err(|error| error.to_string()));
        }
        let Some(result) = &app.audio_health_report else { return };
        match result {
            Ok(report) => {
                ui.label(format!("{} audio clips checked · {} issues", report["audio_clips"], report["issue_count"]));
                ui.label(match report["active_audio_end_seconds"].as_f64() {
                    Some(seconds) => format!("Active audio ends at {seconds:.2} seconds"),
                    None => "Active audio end time is unknown (invalid session sample rate)".into(),
                });
                ui.label("Includes alternate takes and muted clips. Refresh after editing.");
                ui.separator();
                if report["healthy"] == true {
                    ui.label("All audio clips have loaded media within source bounds.");
                }
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    if let Some(issues) = report["issues"].as_array() {
                        for issue in issues {
                            ui.group(|ui| {
                                let text = |key: &str| issue[key].as_str().unwrap_or("");
                                ui.label(format!(
                                    "{} / {} / {}{}",
                                    text("track"),
                                    text("playlist"),
                                    text("clip"),
                                    if issue["active"] == true { "" } else { " (alternate playlist)" }
                                ));
                                ui.label(text("message"));
                                if let Some(path) = issue["path"].as_str() {
                                    ui.label(path);
                                }
                            });
                        }
                    }
                });
                ui.separator();
                ui.label("Checks loaded media only, not files on disk, signal levels, plugins or routing.");
            }
            Err(error) => {
                ui.label(error.to_string());
            }
        }
    });
    app.ui.show_audio_health = open;
    if !open {
        app.audio_health_report = None;
    }
}

fn win(ctx: &egui::Context, open: &mut bool, title: &str, size: egui::Vec2, body: impl FnOnce(&mut egui::Ui)) {
    if !*open {
        return;
    }
    egui::Window::new(title).open(open).default_size(size).show(ctx, body);
}

fn automation(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_automation;
    win(ctx, &mut open, "Automation", vec2(260.0, 260.0), |ui| {
        ui.label("Write automation for the selected tracks:");
        let tracks = app.engine.session().edit.selected_tracks.clone();
        ui.horizontal_wrapped(|ui| {
            for m in [
                AutomationMode::Off,
                AutomationMode::Read,
                AutomationMode::Touch,
                AutomationMode::Latch,
                AutomationMode::TouchLatch,
                AutomationMode::Write,
                AutomationMode::Trim,
            ] {
                if ui.button(m.label()).clicked() {
                    let ids: Vec<u64> = tracks.iter().map(|t| t.0).collect();
                    let _ = app.run("mix.automation_mode", json!({"tracks": ids, "mode": m.label()}));
                }
            }
        });
        ui.separator();
        ui.label("Enabled parameters");
        for f in ["volume", "pan", "mute", "send level", "send pan", "send mute", "plugin"] {
            let id = format!("automation.enable.{}", f.replace(' ', "_"));
            let mut on = !app.engine.session().edit.flag(&format!("{id}.off"));
            if crate::widgets::checkbox(ui, &mut on, f).changed() {
                app.engine.session_mut().edit.set_flag(&format!("{id}.off"), !on);
            }
        }
        ui.separator();
        if ui.button("Write to Current").clicked() {
            let _ = app.run("automation.write_to_current", json!({}));
        }
        if ui.button("Thin All").clicked() {
            let _ = app.run("automation.thin_all", json!({}));
        }
    });
    app.ui.show_automation = open;
}

fn color_palette(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_color_palette;
    win(ctx, &mut open, "Color Palette", vec2(300.0, 140.0), |ui| {
        ui.label("Apply to selected tracks (Shift: selected clips)");
        ui.horizontal_wrapped(|ui| {
            for (i, c) in soundcraft_model::TRACK_COLORS.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
                ui.painter().rect_filled(r, 3.0, rgb(*c));
                if resp.clicked() {
                    if ui.input(|x| x.modifiers.shift) {
                        let _ = app.run("clip.color", json!({"color": c}));
                    } else {
                        let _ = app.run("track.color", json!({"color": i}));
                    }
                }
            }
        });
        ui.separator();
        ui.add(egui::Slider::new(&mut app.ui.track_tint, 0.0..=1.0).show_value(false).text("Track tint in the Edit window"));
    });
    app.ui.show_color_palette = open;
}

fn disk_usage(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_disk_usage;
    win(ctx, &mut open, "Disk Usage", vec2(360.0, 160.0), |ui| {
        let s = app.engine.session();
        let bytes: u64 = s.sources.iter().map(|x| x.frames * u64::from(x.channels) * 4).sum();
        ui.label(format!("Session media: {} files, {:.1} MB in memory (32-bit float)", s.sources.len(), bytes as f64 / 1_048_576.0));
        let per_min = f64::from(s.sample_rate.hz()) * 60.0 * 3.0 / 1_048_576.0;
        ui.label(format!("Recording at 24-bit uses {per_min:.1} MB per mono track-minute"));
        if let Some(p) = &app.engine.path {
            ui.label(format!("Session file: {p}"));
        }
    });
    app.ui.show_disk_usage = open;
}

fn system_usage(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_system_usage;
    win(ctx, &mut open, "System Usage", vec2(320.0, 140.0), |ui| {
        ui.label(format!("UI frame time: {:.1} ms", app.frame_ms));
        ui.label(format!("Audio: {}", app.player.as_ref().map_or("no engine".to_string(), |p| format!("{} @ {} Hz", p.device_name, p.device_rate))));
        let plugins: usize = app.engine.session().tracks.iter().map(|t| t.mixer.inserts.len()).sum();
        ui.label(format!("Active plugin instances: {plugins}"));
        ui.label(format!("Tracks: {}", app.engine.session().tracks.len()));
        ui.label(format!("Undo steps: {}", app.engine.undo_history().len()));
    });
    app.ui.show_system_usage = open;
}

fn task_manager(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_task_manager;
    win(ctx, &mut open, "Task Manager", vec2(320.0, 120.0), |ui| {
        match &app.bounce {
            Some(b) => {
                ui.label(format!("Bouncing {} ({:.0}%)", b.path().display(), b.progress() * 100.0));
                ui.add(egui::ProgressBar::new(b.progress()));
            }
            None => {
                ui.label("No background tasks are running.");
            }
        }
        ui.label(egui::RichText::new("Renders and Process jobs run to completion before returning.").small().color(Tokens::current().text_dim));
    });
    app.ui.show_task_manager = open;
}

fn metadata(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_metadata;
    win(ctx, &mut open, "Metadata Inspector", vec2(380.0, 260.0), |ui| {
        let s = app.engine.session();
        ui.label(egui::RichText::new(&s.name).strong());
        egui::Grid::new("meta").num_columns(2).striped(true).show(ui, |ui| {
            for (k, v) in [
                ("Sample rate", format!("{} Hz", s.sample_rate.hz())),
                ("Bit depth", format!("{:?}", s.bit_depth)),
                ("Timecode rate", s.frame_rate.label().to_string()),
                ("Tracks", s.tracks.len().to_string()),
                ("Clips", s.tracks.iter().map(|t| t.clips().len()).sum::<usize>().to_string()),
                ("Audio files", s.sources.len().to_string()),
                ("Markers", s.markers.len().to_string()),
                ("Length", s.format_position(s.content_end(), soundcraft_time::TimeFormat::MinSecs)),
            ] {
                ui.label(k);
                ui.label(v);
                ui.end_row();
            }
        });
        ui.separator();
        let mut c = s.comments.clone();
        ui.label("Session comments");
        if ui.text_edit_multiline(&mut c).changed() {
            app.engine.session_mut().comments = c;
        }
    });
    app.ui.show_metadata = open;
}

fn event_list(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_event_list;
    win(ctx, &mut open, "MIDI Event List", vec2(420.0, 360.0), |ui| {
        let Some(cid) = crate::midi_editor::target_clip(app) else {
            ui.label("Select a MIDI clip.");
            return;
        };
        let notes = match app.engine.session().find_clip(cid).map(|(_, c)| c.content.clone()) {
            Some(ClipContent::Midi { sequence }) => sequence.notes,
            _ => Vec::new(),
        };
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("evl").num_columns(5).striped(true).show(ui, |ui| {
                for h in ["Start (ticks)", "Note", "Vel", "Length", ""] {
                    ui.label(egui::RichText::new(h).strong());
                }
                ui.end_row();
                for (i, n) in notes.iter().enumerate() {
                    ui.label(egui::RichText::new(n.start.to_string()).font(mono(11.0)));
                    ui.label(soundcraft_midi::note_name(n.pitch));
                    let mut v = i64::from(n.velocity);
                    if ui.add(egui::DragValue::new(&mut v).range(1..=127)).changed() {
                        let _ = app.engine.execute("midi.note_edit", &json!({"clip": cid.0, "index": i, "velocity": v}));
                    }
                    ui.label(n.length.to_string());
                    if ui.small_button("✕").clicked() {
                        let _ = app.run("midi.note_delete", json!({"clip": cid.0, "indices": [i]}));
                    }
                    ui.end_row();
                }
            });
        });
    });
    app.ui.show_event_list = open;
}

fn midi_keyboard(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_midi_keyboard;
    if !open {
        // Closed with a key held under the pointer: let it go.
        if let Some(n) = app.typing.mouse.take() {
            crate::musical_typing::release(app, n);
        }
        return;
    }
    let t = crate::theme::Tokens::current();
    egui::Window::new("Musical Typing").open(&mut open).default_size(vec2(680.0, 150.0)).show(ctx, |ui| {
        ui.horizontal(|ui| {
            let step = app.typing.step;
            if ui
                .add(egui::Button::selectable(!step, "Play"))
                .on_hover_text("Keys play live: armed or monitored instrument tracks hear them and record them")
                .clicked()
            {
                app.typing.step = false;
            }
            if ui
                .add(egui::Button::selectable(step, "Step"))
                .on_hover_text("Keys enter notes into the selected MIDI clip at the insertion point")
                .clicked()
            {
                crate::musical_typing::release_all(app);
                app.typing.step = true;
            }
            ui.separator();
            let mut on = app.typing.capture;
            let shortcut = crate::menus::shortcut_text(app, "ui.musical_typing", ui.ctx().os() == egui::os::OperatingSystem::Mac);
            if ui
                .checkbox(&mut on, "Computer keys")
                .on_hover_text(format!("A S D F… white keys, W E T… black keys, Z/X octave, C/V velocity, Tab sustain ({shortcut})"))
                .changed()
            {
                crate::musical_typing::set_capture(app, on);
            }
            ui.separator();
            // Octave: the note the A key plays, in the app's own names (middle C is C3).
            ui.label(egui::RichText::new("Octave").small().color(t.text_dim));
            // With computer keys on, each button shows the key that does the same.
            let keys = app.typing.capture;
            let label = |sign: &str, key: &str| if keys { format!("{sign} {key}") } else { sign.to_string() };
            if ui.small_button(label("−", "Z")).on_hover_text("An octave down (Z)").clicked() {
                crate::musical_typing::shift_octave(app, -1);
            }
            ui.label(egui::RichText::new(soundcraft_midi::note_name(app.typing.base)).monospace());
            if ui.small_button(label("+", "X")).on_hover_text("An octave up (X)").clicked() {
                crate::musical_typing::shift_octave(app, 1);
            }
            ui.separator();
            ui.label(egui::RichText::new("Velocity").small().color(t.text_dim));
            if ui.small_button(label("−", "C")).on_hover_text("Softer (C)").clicked() {
                crate::musical_typing::shift_velocity(app, -1);
            }
            ui.label(egui::RichText::new(format!("{:>3}", app.typing.velocity)).monospace());
            if ui.small_button(label("+", "V")).on_hover_text("Harder (V)").clicked() {
                crate::musical_typing::shift_velocity(app, 1);
            }
            ui.separator();
            let down = app.typing.sustain;
            if ui
                .add(egui::Button::selectable(down, if keys { "Sustain · Tab" } else { "Sustain" }))
                .on_hover_text("The sustain pedal (MIDI CC 64): click to hold it down or lift it; with computer keys on, hold Tab")
                .clicked()
            {
                crate::musical_typing::set_sustain(app, !down);
            }
        });
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(400.0), 80.0), Sense::click_and_drag());
        let whites = 28;
        let w = r.width() / whites as f32;
        let base = app.typing.base.saturating_sub(12).min(80);
        let sounding = crate::musical_typing::sounding(app);
        let lit = Color32::from_rgb(110, 170, 240);
        let black = |p: u8| matches!(p % 12, 1 | 3 | 6 | 8 | 10);
        // The key under the pointer: a black key first (they sit on top).
        let mut keys: Vec<(u8, egui::Rect)> = Vec::new();
        let mut wi = 0;
        for p in base..base.saturating_add(48) {
            if black(p) {
                keys.push((p, egui::Rect::from_min_size(egui::pos2(r.min.x + wi as f32 * w - w * 0.3, r.min.y), vec2(w * 0.6, r.height() * 0.6))));
            } else {
                keys.push((p, egui::Rect::from_min_size(egui::pos2(r.min.x + wi as f32 * w, r.min.y), vec2(w - 1.0, r.height()))));
                wi += 1;
            }
        }
        let typing_letter = |p: u8| {
            crate::musical_typing::NOTE_KEYS
                .iter()
                .find(|(_, d)| app.typing.base.checked_add(*d) == Some(p))
                .map(|(k, _)| k.symbol_or_name().to_string())
        };
        for (p, kr) in keys.iter().filter(|(p, _)| !black(*p)) {
            let fill = if sounding.contains(p) { lit } else { Color32::from_rgb(235, 235, 235) };
            ui.painter().rect_filled(*kr, 2.0, fill);
            if app.typing.capture
                && let Some(l) = typing_letter(*p)
            {
                ui.painter().text(
                    egui::pos2(kr.center().x, kr.max.y - 9.0),
                    egui::Align2::CENTER_CENTER,
                    l,
                    egui::FontId::proportional(10.0),
                    Color32::from_rgb(90, 90, 90),
                );
            }
        }
        for (p, kr) in keys.iter().filter(|(p, _)| black(*p)) {
            let fill = if sounding.contains(p) { lit } else { Color32::from_rgb(20, 20, 20) };
            ui.painter().rect_filled(*kr, 2.0, fill);
            if app.typing.capture
                && let Some(l) = typing_letter(*p)
            {
                ui.painter().text(
                    egui::pos2(kr.center().x, kr.max.y - 8.0),
                    egui::Align2::CENTER_CENTER,
                    l,
                    egui::FontId::proportional(9.0),
                    Color32::from_rgb(200, 200, 200),
                );
            }
        }
        let under = |q: egui::Pos2| {
            keys.iter().filter(|(p, _)| black(*p)).chain(keys.iter().filter(|(p, _)| !black(*p))).find(|(_, kr)| kr.contains(q)).map(|(p, _)| *p)
        };
        // Held down: the key sounds until the pointer lets go (or slides to another key).
        let pressed = if resp.is_pointer_button_down_on() { resp.interact_pointer_pos().and_then(under) } else { None };
        if pressed != app.typing.mouse {
            let was = app.typing.mouse.take();
            if let Some(n) = was {
                crate::musical_typing::release(app, n);
            }
            if let Some(n) = pressed {
                // Step entry takes the key pressed, not every key slid over.
                if !app.typing.step || was.is_none() {
                    crate::musical_typing::press(app, n);
                }
                app.typing.mouse = Some(n);
            }
        }
        let hint = if app.typing.step {
            "Step: click keys (or type them) to enter notes into the selected MIDI clip at the insertion point."
        } else {
            "Play: arm an instrument track on MIDI: All Inputs (or Musical Typing) and play; Record records it."
        };
        ui.label(egui::RichText::new(hint).small());
        // The computer keys, spelled out (not only in tooltips).
        let keys_hint = if app.typing.capture {
            "Computer keys: A S D F G H J K L ; white keys, W E T Y U O P black keys · Z X octave · C V velocity · Tab sustain (held)".to_string()
        } else {
            let shortcut = crate::menus::shortcut_text(app, "ui.musical_typing", ui.ctx().os() == egui::os::OperatingSystem::Mac);
            format!("Tick Computer keys ({shortcut}) to play from your computer keyboard.")
        };
        ui.label(egui::RichText::new(keys_hint).small().color(t.text_dim));
    });
    if !open {
        // Closed: the keys it played go back to their shortcuts.
        crate::musical_typing::release_all(app);
        crate::musical_typing::set_capture(app, false);
    }
    app.ui.show_midi_keyboard = open;
}

fn workspace(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_workspace;
    if !open {
        return;
    }
    egui::Window::new("Workspace").open(&mut open).default_size(vec2(460.0, 420.0)).show(ctx, |ui| {
        let dir = if app.ui.workspace_dir.is_empty() { std::env::var("HOME").unwrap_or_else(|_| ".".into()) } else { app.ui.workspace_dir.clone() };
        ui.horizontal(|ui| {
            if ui.button("⬆").on_hover_text("Parent folder").clicked()
                && let Some(p) = std::path::Path::new(&dir).parent()
            {
                app.ui.workspace_dir = p.to_string_lossy().into_owned();
            }
            ui.label(egui::RichText::new(&dir).small());
        });
        ui.separator();
        let mut entries: Vec<(String, bool)> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        if name.starts_with('.') {
                            return None;
                        }
                        let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
                        let ext = std::path::Path::new(&name).extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
                        (is_dir
                            || ["wav", "aif", "aiff", "flac", "mp3", "ogg", "opus", "m4a", "m4b", "caf", "mid", "midi"].contains(&ext.as_str())
                            || soundcraft_model::is_session_extension(&ext))
                        .then_some((name, is_dir))
                    })
                    .collect()
            })
            .unwrap_or_default();
        entries.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (name, is_dir) in entries.into_iter().take(2000) {
                let path = std::path::Path::new(&dir).join(&name).to_string_lossy().into_owned();
                let label = if is_dir { format!("📁 {name}") } else { format!("🔊 {name}") };
                let r = ui.selectable_label(false, label);
                if r.double_clicked() {
                    if is_dir {
                        app.ui.workspace_dir = path;
                    } else if soundcraft_model::is_session_path(&name) {
                        let _ = crate::dialogs::run_or_ask(app, "session.open", json!({"path": path}));
                    } else if name.ends_with(".mid") || name.ends_with(".midi") {
                        let _ = app.run("file.import_midi", json!({"path": path}));
                    } else {
                        let at = app.engine.session().edit.selection.start;
                        let _ = app.run("file.import_audio", json!({"path": path, "at": at}));
                    }
                }
            }
        });
        ui.label(egui::RichText::new("Double-click to import audio/MIDI at the insertion point or open a session.").small());
    });
    app.ui.show_workspace = open;
}

fn configurations(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_configurations;
    win(ctx, &mut open, "Window Configurations", vec2(320.0, 240.0), |ui| {
        if ui.button("New Configuration…").clicked() {
            let n = app.ui.configurations.len() + 1;
            let mut snap = app.ui.clone();
            snap.configurations.clear();
            if let Ok(v) = serde_json::to_value(&snap) {
                app.ui.configurations.push((format!("Configuration {n}"), v));
            }
        }
        ui.separator();
        let list = app.ui.configurations.clone();
        for (i, (name, v)) in list.iter().enumerate() {
            ui.horizontal(|ui| {
                if ui.button(format!("{}  {name}", i + 1)).clicked()
                    && let Ok(mut st) = serde_json::from_value::<crate::UiState>(v.clone())
                {
                    if let Err(error) = st.docking.validate().and_then(|()| st.mix_docking.validate_mix()) {
                        app.ui.status = format!("Window configuration could not be restored: {error}");
                    } else {
                        st.configurations = app.ui.configurations.clone();
                        st.theme = app.ui.theme;
                        // The sections already offered stay offered (an older configuration would offer them again).
                        st.mix_views_offered = app.ui.mix_views_offered.clone();
                        st.show_configurations = true;
                        crate::docking::cancel_replaced_state(app, &st);
                        app.ui = st;
                    }
                }
                if ui.small_button("✕").clicked() {
                    app.ui.configurations.remove(i);
                }
            });
        }
    });
    app.ui.show_configurations = open;
}

/// The audio systems as last enumerated (enumerating probes devices: done when the window opens
/// and on Refresh, not every frame).
fn audio_systems(ctx: &egui::Context, refresh: bool) -> Arc<Vec<AudioSystem>> {
    let id = egui::Id::new("audio_systems");
    if !refresh && let Some(s) = ctx.data(|d| d.get_temp::<Arc<Vec<AudioSystem>>>(id)) {
        return s;
    }
    let s = Arc::new(soundcraft_playback::devices::audio_systems());
    ctx.data_mut(|d| d.insert_temp(id, Arc::clone(&s)));
    s
}

/// The system a config means (its own, else the default one) in an enumeration.
fn chosen_system<'a>(systems: &'a [AudioSystem], cfg: &AudioConfig) -> Option<&'a AudioSystem> {
    cfg.system
        .as_deref()
        .and_then(|n| systems.iter().find(|s| s.name.eq_ignore_ascii_case(n)))
        .or_else(|| systems.iter().find(|s| s.is_default))
        .or(systems.first())
}

/// The device a config means (by name, else the system default).
fn chosen_device<'a>(devices: &'a [DeviceInfo], name: Option<&str>) -> Option<&'a DeviceInfo> {
    name.and_then(|n| devices.iter().find(|d| d.name == n)).or_else(|| devices.iter().find(|d| d.is_default))
}

fn device_combo(ui: &mut egui::Ui, id: &str, value: &mut Option<String>, devices: &[DeviceInfo]) {
    let default = devices.iter().find(|d| d.is_default).map_or("none".to_string(), |d| d.name.clone());
    let shown = value.clone().unwrap_or_else(|| format!("Default ({default})"));
    egui::ComboBox::from_id_salt(id).width(300.0).selected_text(shown).show_ui(ui, |ui| {
        ui.selectable_value(value, None, format!("Default ({default})"));
        for d in devices {
            ui.selectable_value(value, Some(d.name.clone()), format!("{} ({} ch)", d.name, d.max_channels));
        }
    });
}

fn number_combo(ui: &mut egui::Ui, id: &str, value: &mut Option<u32>, options: &[u32], unit: &str) {
    let shown = value.map_or("Device default".to_string(), |v| format!("{v} {unit}"));
    egui::ComboBox::from_id_salt(id).width(300.0).selected_text(shown).show_ui(ui, |ui| {
        ui.selectable_value(value, None, "Device default");
        for v in options {
            ui.selectable_value(value, Some(*v), format!("{v} {unit}"));
        }
    });
}

/// Setup › Playback Engine / Hardware: choose the audio system, devices, rate and buffer size.
pub fn playback_engine(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_playback_engine;
    let was_open = egui::Id::new("pe-was-open");
    if !open {
        // Closed some other way than its own ✕ (a command, Hide All) mid-measure: nothing would
        // show the result.
        if ctx.data_mut(|d| d.remove_temp::<bool>(was_open)).is_some() && app.measuring() {
            app.cancel_measure();
        }
        return;
    }
    ctx.data_mut(|d| d.insert_temp(was_open, true));
    let draft_id = egui::Id::new("audio_draft");
    let mut draft = ctx.data(|d| d.get_temp::<AudioConfig>(draft_id)).unwrap_or_else(|| app.ui.audio.clone());
    let mut refresh = false;
    let mut apply = false;
    let systems = audio_systems(ctx, false);
    win(ctx, &mut open, "Playback Engine", vec2(460.0, 360.0), |ui| {
        egui::Grid::new("pe").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
            ui.label("Audio system");
            let def = systems.iter().find(|s| s.is_default).map_or("none".to_string(), |s| s.name.clone());
            let shown = draft.system.clone().unwrap_or_else(|| format!("Default ({def})"));
            egui::ComboBox::from_id_salt("pe-system").width(300.0).selected_text(shown).show_ui(ui, |ui| {
                let before = draft.system.clone();
                ui.selectable_value(&mut draft.system, None, format!("Default ({def})"));
                for s in systems.iter() {
                    ui.selectable_value(&mut draft.system, Some(s.name.clone()), &s.name);
                }
                if draft.system != before {
                    // Device names belong to a system.
                    draft.output = None;
                    draft.input = None;
                }
            });
            ui.end_row();
            let sys = chosen_system(&systems, &draft);
            ui.label("Output device");
            device_combo(ui, "pe-out", &mut draft.output, sys.map_or(&[][..], |s| &s.outputs));
            ui.end_row();
            ui.label("Input device");
            device_combo(ui, "pe-in", &mut draft.input, sys.map_or(&[][..], |s| &s.inputs));
            ui.end_row();
            let out = sys.and_then(|s| chosen_device(&s.outputs, draft.output.as_deref()));
            ui.label("Sample rate");
            number_combo(ui, "pe-rate", &mut draft.sample_rate, out.map_or(&[][..], |d| &d.sample_rates), "Hz");
            ui.end_row();
            ui.label("Buffer size");
            let buffers = out.map(DeviceInfo::buffer_sizes).unwrap_or_else(|| soundcraft_playback::devices::STANDARD_BUFFERS.to_vec());
            number_combo(ui, "pe-buffer", &mut draft.buffer_size, &buffers, "samples");
            ui.end_row();
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Refresh devices").clicked() {
                refresh = true;
            }
            let changed = draft != app.ui.audio;
            if ui.add_enabled(changed, egui::Button::new("Apply")).on_hover_text("Reopen the audio devices with these settings").clicked() {
                apply = true;
            }
            if changed && ui.button("Revert").clicked() {
                draft = app.ui.audio.clone();
            }
        });
        ui.separator();
        egui::Grid::new("pe-state").num_columns(2).show(ui, |ui| {
            ui.label("Output");
            ui.label(app.player.as_ref().map_or("none".to_string(), |p| {
                if p.silent {
                    "no device (silent clock)".to_string()
                } else {
                    let buf = p.device_buffer.map_or("default buffer".to_string(), |b| format!("{b} samples"));
                    format!("{} · {} Hz · {} ch · {buf}", p.device_name, p.device_rate, p.device_channels)
                }
            }));
            ui.end_row();
            ui.label("Input");
            ui.label(app.recorder.as_ref().map_or("opened on first record or monitor".to_string(), |r| {
                format!("{} · {} Hz · {} ch", r.device_name, r.device_rate, r.channels)
            }));
            ui.end_row();
            ui.label("Session rate");
            let sr = app.engine.session().sample_rate.hz();
            let note = app
                .player
                .as_ref()
                .filter(|p| !p.silent && p.device_rate != sr)
                .map_or(String::new(), |_| " (resampled to the device rate)".to_string());
            ui.label(format!("{sr} Hz{note}"));
            ui.end_row();
        });
        ui.separator();
        engine_status(app, ui);
        ui.separator();
        let mut dc = app.engine.session().edit.delay_compensation;
        if crate::widgets::checkbox(ui, &mut dc, "Delay compensation").changed() {
            let _ = app.run("options.delay_compensation", json!({"value": dc}));
        }
        if ui.button("Reconnect audio device").clicked() {
            app.reconnect_audio();
        }
    });
    if refresh {
        audio_systems(ctx, true);
    }
    if apply && let Err(e) = app.apply_audio_config(draft.clone()) {
        app.ui.status = e;
    }
    if !open {
        ctx.data_mut(|d| d.remove_temp::<bool>(was_open));
        if app.measuring() {
            // Closed mid-measure: nothing would show its result.
            app.cancel_measure();
        }
    }
    if open {
        ctx.data_mut(|d| d.insert_temp(draft_id, draft));
    } else {
        // Closing discards unapplied edits and the device list (re-read on the next open).
        ctx.data_mut(|d| {
            d.remove_temp::<AudioConfig>(draft_id);
            d.remove_temp::<Arc<Vec<AudioSystem>>>(egui::Id::new("audio_systems"));
        });
    }
    app.ui.show_playback_engine = open;
}

/// `n` samples at `rate` in milliseconds, for display ("6.3 ms").
fn ms(n: i64, rate: u32) -> String {
    format!("{:.1} ms", n as f64 * 1000.0 / f64::from(rate.max(1)))
}

/// The Playback Engine window's live part: the mix block, the reported latencies, the record
/// offset with Measure and the manual offset, the underrun counter and the DSP load, and the rate
/// warning.
fn engine_status(app: &mut SoundApp, ui: &mut egui::Ui) {
    let t = Tokens::current();
    let Some(p) = app.player.as_ref() else {
        ui.label(egui::RichText::new("No audio engine").color(t.text_dim));
        return;
    };
    let session_rate = app.engine.session().sample_rate.hz();
    // Equal rates render the device's frames 1:1 (the direct path; half speed resamples for a
    // while, which isn't the setup).
    let block = if p.device_rate == session_rate {
        match p.callback_frames().or(p.device_buffer) {
            Some(n) => format!("{n} samples (the device buffer)"),
            None => "the device buffer".to_string(),
        }
    } else {
        format!("Mix resamples {} Hz → {session_rate} Hz", p.device_rate)
    };
    let l = p.reported_latency();
    let (underruns, silent) = (p.underruns(), p.silent);
    // The peak load since the last frame, held and eased down so it can be read.
    let load_id = egui::Id::new("pe-load");
    let held = ui.ctx().data(|d| d.get_temp::<f32>(load_id)).unwrap_or(0.0);
    let load = p.dsp_load().max(held * 0.97);
    ui.ctx().data_mut(|d| d.insert_temp(load_id, load));
    // Live figures: keep them moving while the window is open.
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    let offset = app.record_offset();
    let total = app.record_offset_total();
    let measuring = app.measuring();
    let mut run: Option<(&str, serde_json::Value)> = None;
    let mut cancel = false;
    egui::Grid::new("pe-engine").num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
        ui.label("Mix block");
        ui.label(block);
        ui.end_row();
        ui.label("Reported latency");
        let lat = |n: u32| format!("{n} samples ({})", ms(i64::from(n), session_rate));
        ui.label(format!("in {} · out {}", lat(l.input), lat(l.output)));
        ui.end_row();
        ui.label("Record offset");
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{total} samples ({})", ms(total, session_rate))).strong());
            if offset.measured.is_some() {
                ui.label(egui::RichText::new("(measured)").color(t.meter_green));
            } else {
                ui.label(egui::RichText::new(crate::record_offset::NOT_MEASURED).color(t.meter_yellow));
            }
        })
        .response
        .on_hover_text(
            "Takes are placed this much earlier: the round trip (measured, else as the devices report it) + the manual offset + the mix's own delay",
        );
        ui.end_row();
        ui.label("Manual offset");
        let mut manual = offset.manual;
        let drag = egui::DragValue::new(&mut manual)
            .range(-crate::record_offset::MAX_MANUAL..=crate::record_offset::MAX_MANUAL)
            .suffix(" samples")
            .speed(1.0);
        if ui.add(drag).on_hover_text("Added to the round trip for this device, buffer and rate").changed() {
            run = Some(("setup.record_offset", json!({"samples": manual})));
        }
        ui.end_row();
        ui.label("Measure");
        ui.horizontal(|ui| {
            let (dev_in, dev_out) = app.device_channels();
            let s = app.engine.session();
            let outputs = crate::mix_window::output_choices(s, dev_out);
            let inputs = crate::mix_window::input_choices(s, dev_in);
            let pick_id = egui::Id::new("pe-measure-pick");
            let (mut out_pick, mut in_pick) = ui.ctx().data(|d| d.get_temp::<(usize, usize)>(pick_id)).unwrap_or((0, 0));
            out_pick = out_pick.min(outputs.len().saturating_sub(1));
            in_pick = in_pick.min(inputs.len().saturating_sub(1));
            let label =
                |c: Option<&(String, bool)>| c.map_or("none".to_string(), |(n, ok)| if *ok { n.clone() } else { format!("{n} (not available)") });
            egui::ComboBox::from_id_salt("pe-measure-out").width(110.0).selected_text(label(outputs.get(out_pick))).show_ui(ui, |ui| {
                for (i, c) in outputs.iter().enumerate() {
                    ui.selectable_value(&mut out_pick, i, label(Some(c)));
                }
            });
            ui.label("→");
            egui::ComboBox::from_id_salt("pe-measure-in").width(110.0).selected_text(label(inputs.get(in_pick))).show_ui(ui, |ui| {
                for (i, c) in inputs.iter().enumerate() {
                    ui.selectable_value(&mut in_pick, i, label(Some(c)));
                }
            });
            ui.ctx().data_mut(|d| d.insert_temp(pick_id, (out_pick, in_pick)));
            if measuring {
                ui.spinner();
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            } else {
                let tip = "Patch the output to the input (a cable, or the X32's internal routing), then measure the true round trip at this buffer";
                if ui.add_enabled(!silent, egui::Button::new("Measure")).on_hover_text(tip).clicked() {
                    let mut params = json!({});
                    // The main by default (no name: also when the session names no outputs).
                    if out_pick > 0
                        && let Some((name, _)) = outputs.get(out_pick)
                    {
                        params["output"] = json!(name);
                    }
                    if let Some((name, _)) = inputs.get(in_pick) {
                        params["input"] = json!(name);
                    }
                    run = Some(("setup.measure_latency", params));
                }
            }
        });
        ui.end_row();
        ui.label("Underruns");
        ui.horizontal(|ui| {
            let color = if underruns > 0 { t.meter_red } else { t.text };
            ui.label(egui::RichText::new(underruns.to_string()).color(color));
            if ui.add_enabled(underruns > 0, egui::Button::new("Reset").small()).clicked() {
                run = Some(("ui.reset_underruns", json!({})));
            }
        });
        ui.end_row();
        ui.label("DSP load");
        ui.label(format!("{:.0}%", load * 100.0));
        ui.end_row();
        // Audio played from disk: blocks the audio thread found not read yet (each a short
        // silence on one clip), and how much of the disk cache holds decoded audio.
        let late = soundcraft_model::disk::underruns();
        ui.label("Disk too slow");
        ui.label(egui::RichText::new(late.to_string()).color(if late > 0 { t.meter_red } else { t.text }))
            .on_hover_text("Blocks of audio played from disk that were not read in time since FreeDAWm started (each plays as a short silence)");
        ui.end_row();
        let (held, limit) = soundcraft_model::disk::usage();
        ui.label("Disk cache");
        ui.label(format!("{:.0} of {:.0} MB", held as f64 / 1e6, limit as f64 / 1e6));
        ui.end_row();
    });
    if let Some(w) = app.rate_warning() {
        ui.label(egui::RichText::new(w).color(t.meter_yellow));
    }
    if cancel {
        app.cancel_measure();
    }
    if let Some((id, params)) = run
        && let Err(e) = app.run(id, params)
    {
        app.ui.status = e;
    }
}

/// `ui.audio_device`: `{list?: bool}` returns the audio systems and their devices;
/// `{system?, output?, input?, sample_rate?, buffer_size?}` (a `null` value means "default") sets
/// the choice and reopens the devices. Always returns the choice and the open device.
pub fn audio_device_command(app: &mut SoundApp, p: &serde_json::Value) -> Result<serde_json::Value, String> {
    use serde_json::Value;
    let mut changed = false;
    let mut cfg = app.ui.audio.clone();
    for (key, slot) in [("system", &mut cfg.system), ("output", &mut cfg.output), ("input", &mut cfg.input)] {
        if let Some(v) = p.get(key) {
            *slot = v.as_str().map(str::to_string).filter(|s| !s.is_empty());
            changed = true;
        }
    }
    for (key, slot, range) in [("sample_rate", &mut cfg.sample_rate, 8_000..=768_000u64), ("buffer_size", &mut cfg.buffer_size, 16..=8192u64)] {
        if let Some(v) = p.get(key) {
            *slot = match v {
                Value::Null => None,
                v => Some(v.as_u64().filter(|n| range.contains(n)).ok_or_else(|| format!("`{key}` must be a number in {range:?} or null"))? as u32),
            };
            changed = true;
        }
    }
    if changed {
        app.apply_audio_config(cfg)?;
    }
    let device = app.player.as_ref().map(|pl| {
        json!({"name": pl.device_name, "sample_rate": pl.device_rate, "channels": pl.device_channels, "buffer_size": pl.device_buffer, "silent": pl.silent})
    });
    let mut out = json!({"config": app.ui.audio, "device": device});
    if p.get("list").and_then(Value::as_bool).unwrap_or(false) {
        out["systems"] = json!(soundcraft_playback::devices::audio_systems());
    }
    Ok(out)
}

/// Setup › I/O: named inputs, busses and outputs. Every edit is a `setup.io` action (one undo
/// step each; a channel drag merges into one). A path the open device lacks reads "(not available)".
pub fn io_setup(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_io_setup;
    if !open {
        return;
    }
    let (dev_in, dev_out) = app.device_channels();
    win(ctx, &mut open, "I/O Setup", vec2(560.0, 540.0), |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            io_inputs(app, ui, dev_in);
            ui.separator();
            io_busses(app, ui);
            ui.separator();
            io_outputs(app, ui, dev_out);
        });
    });
    app.ui.show_io_setup = open;
}

/// A section heading with the open device's channel count after it.
fn io_heading(ui: &mut egui::Ui, title: &str, device: Option<(&str, Option<usize>)>) {
    let t = Tokens::current();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).strong());
        if let Some((what, n)) = device {
            let note = n.map_or(format!("{what} device not open"), |n| format!("{what} device: {n} ch"));
            ui.label(egui::RichText::new(note).small().color(t.text_dim));
        }
    });
}

/// A path's name field: renames through `setup.io` when it loses focus. A refused name (taken,
/// reserved) puts the old one back and says why in the status bar.
fn io_name_field(app: &mut SoundApp, ui: &mut egui::Ui, kind: &str, name: &str, action: &str) {
    let key = egui::Id::new((kind, "name", name.to_string()));
    let mut edit: String = ui.ctx().memory(|m| m.data.get_temp(key)).unwrap_or_else(|| name.to_string());
    let r = ui.add_sized(vec2(150.0, ui.spacing().interact_size.y), egui::TextEdit::singleline(&mut edit));
    if r.lost_focus() && edit != name {
        if let Err(e) = app.run("setup.io", json!({"action": action, "name": name, "new_name": edit})) {
            app.ui.status = e;
        }
        ui.ctx().memory_mut(|m| m.data.remove::<String>(key));
    } else {
        ui.ctx().memory_mut(|m| m.data.insert_temp(key, edit));
    }
}

/// The 1-based first-channel field and the range it covers, then "not available" when the open
/// device lacks it. A drag is one undo step.
fn io_channel_field(app: &mut SoundApp, ui: &mut egui::Ui, name: &str, first: u16, width: usize, action: &str, fits: bool) {
    let t = Tokens::current();
    let mut ch = u32::from(first) + 1;
    let r = ui.add(egui::DragValue::new(&mut ch).range(1..=1024).speed(0.1).prefix("ch "));
    if r.changed() {
        let params = json!({"action": action, "name": name, "first_channel": ch.saturating_sub(1)});
        if let Err(e) = app.engine.execute_merged("setup.io", &params, &format!("io-channel:{action}:{name}")) {
            app.ui.status = e.to_string();
        }
    }
    // A multichannel path's span ("ch 5" covers 5–6).
    let span = if width > 1 { format!("{ch}–{}", ch as usize + width - 1) } else { String::new() };
    ui.label(egui::RichText::new(span).color(t.text_dim));
    if fits {
        ui.label("");
    } else {
        ui.label(egui::RichText::new(crate::mix_window::NOT_AVAILABLE.trim()).color(t.rec));
    }
}

/// Mono or stereo, for an input or a hardware output.
fn io_format_combo(app: &mut SoundApp, ui: &mut egui::Ui, kind: &str, name: &str, format: ChannelFormat, action: &str) {
    egui::ComboBox::from_id_salt((kind, "format", name.to_string())).width(72.0).selected_text(format.label()).show_ui(ui, |ui| {
        for f in [ChannelFormat::Mono, ChannelFormat::Stereo] {
            if ui.selectable_label(f == format, f.label()).clicked() && f != format {
                let _ = app.run("setup.io", json!({"action": action, "name": name, "format": f.label()}));
            }
        }
    });
}

fn io_inputs(app: &mut SoundApp, ui: &mut egui::Ui, device: Option<usize>) {
    io_heading(ui, "Inputs", Some(("Input", device)));
    let inputs = app.engine.session().inputs.clone();
    if inputs.is_empty() {
        let t = Tokens::current();
        ui.label(egui::RichText::new("No named inputs: tracks use In 1, In 2…").small().color(t.text_dim));
    }
    egui::Grid::new("io_inputs").num_columns(6).striped(true).spacing(vec2(8.0, 4.0)).show(ui, |ui| {
        for i in &inputs {
            let width = i.format.channels().clamp(1, 2);
            io_name_field(app, ui, "input", &i.name, "rename_input");
            io_format_combo(app, ui, "input", &i.name, i.format, "set_input_format");
            let fits = crate::mix_window::path_fits(usize::from(i.first_channel), width, device);
            io_channel_field(app, ui, &i.name, i.first_channel, width, "set_input_channel", fits);
            if ui.small_button("Delete").clicked() {
                let _ = app.run("setup.io", json!({"action": "delete_input", "name": i.name}));
            }
            ui.end_row();
        }
    });
    ui.horizontal(|ui| {
        if ui.button("New Input").clicked() {
            let s = app.engine.session();
            let mut n = s.inputs.len() + 1;
            while s.inputs.iter().any(|i| soundcraft_model::same_name(&i.name, &format!("Input {n}"))) {
                n += 1;
            }
            // The channel after the last one used.
            let next = s.inputs.iter().map(|i| usize::from(i.first_channel) + i.format.channels().clamp(1, 2)).max().unwrap_or(0);
            let _ = app.run("setup.io", json!({"action": "create_input", "name": format!("Input {n}"), "format": "mono", "first_channel": next}));
        }
        let n = device.filter(|n| *n > 0).unwrap_or(8).min(64);
        if ui.button(format!("Add In 1–{n}")).on_hover_text("A mono input per device channel (names that exist are kept)").clicked() {
            let _ = app.run("setup.io", json!({"action": "auto_inputs", "channels": n}));
        }
    });
}

fn io_busses(app: &mut SoundApp, ui: &mut egui::Ui) {
    io_heading(ui, "Busses", None);
    let busses: Vec<(String, String)> = app.engine.session().busses.iter().map(|b| (b.name.clone(), b.format.label().to_string())).collect();
    egui::Grid::new("busses").num_columns(3).striped(true).show(ui, |ui| {
        for (name, fmt) in busses {
            let key = egui::Id::new(("bus_name", name.clone()));
            let mut edit: String = ui.ctx().memory(|m| m.data.get_temp(key)).unwrap_or_else(|| name.clone());
            let r = ui.text_edit_singleline(&mut edit);
            ui.ctx().memory_mut(|m| m.data.insert_temp(key, edit.clone()));
            if r.lost_focus() && edit != name {
                let _ = app.run("setup.io", json!({"action": "rename_bus", "name": name, "new_name": edit}));
            }
            ui.label(fmt);
            if ui.small_button("Delete").clicked() {
                let _ = app.run("setup.io", json!({"action": "delete_bus", "name": name}));
            }
            ui.end_row();
        }
    });
    if ui.button("New Bus").clicked() {
        let n = app.engine.session().busses.len() + 1;
        let _ = app.run("mix.new_bus", json!({"name": format!("Bus {n}")}));
    }
}

fn io_outputs(app: &mut SoundApp, ui: &mut egui::Ui, device: Option<usize>) {
    io_heading(ui, "Outputs", Some(("Output", device)));
    let outputs = app.engine.session().outputs.clone();
    let choices = crate::mix_window::output_choices(app.engine.session(), device);
    let main = app.engine.session().main_format();
    egui::Grid::new("io_outputs").num_columns(6).striped(true).spacing(vec2(8.0, 4.0)).show(ui, |ui| {
        for (i, o) in outputs.iter().enumerate() {
            let fits = choices.get(i).is_none_or(|c| c.1);
            io_name_field(app, ui, "output", &o.name, "rename_output");
            if i == 0 {
                // The main's format is the session's main format (the mix and master follow it).
                egui::ComboBox::from_id_salt("main_output_format").width(72.0).selected_text(main.label()).show_ui(ui, |ui| {
                    for f in ChannelFormat::ALL.into_iter().filter(|f| (2..=16).contains(&f.channels())) {
                        if ui.selectable_label(f == main, f.label()).clicked()
                            && f != main
                            && let Ok(r) = app.run("setup.main_format", json!({"format": f.label()}))
                            && let Some(note) = r["note"].as_str()
                        {
                            app.ui.status = note.to_string();
                        }
                    }
                });
                io_channel_field(app, ui, &o.name, o.first_channel, main.channels(), "set_output_channel", fits);
                ui.label(egui::RichText::new("main").small().color(Tokens::current().text_dim));
            } else {
                io_format_combo(app, ui, "output", &o.name, o.format, "set_output_format");
                io_channel_field(app, ui, &o.name, o.first_channel, o.format.channels(), "set_output_channel", fits);
                if ui.small_button("Delete").clicked() {
                    let _ = app.run("setup.io", json!({"action": "delete_output", "name": o.name}));
                }
            }
            ui.end_row();
        }
    });
    if ui.button("New Output").clicked() {
        let s = app.engine.session();
        let next = s.outputs.iter().map(|o| usize::from(o.first_channel) + o.format.channels()).max().unwrap_or(2).max(2);
        let mut name = format!("Out {}-{}", next + 1, next + 2);
        let mut k = 2;
        while s.outputs.iter().any(|o| soundcraft_model::same_name(&o.name, &name)) {
            name = format!("Out {}-{} ({k})", next + 1, next + 2);
            k += 1;
        }
        let _ = app.run("setup.io", json!({"action": "create_output", "name": name, "format": "stereo", "first_channel": next}));
    }
}

/// Setup › Keyboard Shortcuts (the editor lives in `shortcut_editor`).
pub fn shortcuts_window(app: &mut SoundApp, ctx: &egui::Context) {
    crate::shortcut_editor::window(app, ctx);
    crate::preferences::window(app, ctx);
}

/// Clip Effects: per-clip EQ, dynamics and gain (stored as `clip_fx.<clip>.*` values).
pub fn clip_effects(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_clip_effects;
    win(ctx, &mut open, "Clip Effects", vec2(560.0, 420.0), |ui| {
        let clip = app.engine.session().edit.selected_clips.first().copied().or_else(|| {
            let s = app.engine.session();
            s.tracks
                .iter()
                .filter(|t| s.edit.selected_tracks.contains(&t.id))
                .flat_map(|t| t.clips().iter())
                .find(|c| c.range().contains(s.edit.selection.start) && c.is_audio())
                .map(|c| c.id)
        });
        let Some(cid) = clip else {
            ui.label("Select an audio clip.");
            return;
        };
        let name = app.engine.session().find_clip(cid).map(|(_, c)| c.name.clone()).unwrap_or_default();
        let bypass = app.engine.session().edit.flag(&format!("clip_fx.bypass.{}", cid.0));
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&name).strong());
            if ui.selectable_label(bypass, "Bypass").clicked() {
                let _ = app.run("clip.effects_bypass", json!({"clips": [cid.0]}));
            }
            if ui.button("Render").clicked() {
                let _ = app.run("clip.effects_render", json!({"clips": [cid.0]}));
            }
            if ui.button("Clear").clicked() {
                let _ = app.run("edit.clear_clip_effects", json!({"clips": [cid.0]}));
            }
        });
        let get = |app: &SoundApp, k: &str, d: f32| app.engine.session().edit.values.get(&format!("clip_fx.{}.{k}", cid.0)).map_or(d, |v| *v as f32);
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.columns(2, |cols| {
                for (col, (title, plugin, prefix)) in cols.iter_mut().zip([("EQ", "eq_7band", "eq."), ("Dynamics", "compressor", "comp.")]) {
                    col.label(egui::RichText::new(title).strong());
                    let Some(info) = soundcraft_dsp::plugin_info(plugin) else { continue };
                    for p in info.params {
                        let key = format!("{prefix}{}", p.id);
                        let mut v = get(app, &key, p.default);
                        let before = v;
                        col.add(egui::Slider::new(&mut v, p.min..=p.max).text(p.name));
                        if (v - before).abs() > f32::EPSILON {
                            let mut m = serde_json::Map::new();
                            m.insert(key.clone(), json!(v));
                            let _ = app.engine.execute_merged(
                                "clip.effects_set",
                                &json!({"clips": [cid.0], "params": m}),
                                &format!("cfx:{}:{key}", cid.0),
                            );
                        }
                    }
                }
            });
            let mut g = get(app, "gain", 0.0);
            let before = g;
            ui.add(egui::Slider::new(&mut g, -36.0..=24.0).text("Clip effects gain (dB)"));
            if (g - before).abs() > f32::EPSILON {
                let _ =
                    app.engine.execute_merged("clip.effects_set", &json!({"clips": [cid.0], "params": {"gain": g}}), &format!("cfx:{}:gain", cid.0));
            }
        });
    });
    app.ui.show_clip_effects = open;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod audio_health_tests {
    use super::*;

    #[test]
    fn window_command_renders_report_and_close_clears_snapshot() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, crate::Services::default());
        app.run("window.audio_health", json!({"value": true})).unwrap();
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| audio_health(&mut app, ui.ctx()));
        output.textures_delta.clear();
        assert_eq!(app.audio_health_report.as_ref().unwrap().as_ref().unwrap()["healthy"], true);
        assert!(!app.engine.is_dirty());
        app.run("window.hide_floating", json!({})).unwrap();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| audio_health(&mut app, ui.ctx()));
        output.textures_delta.clear();
        assert!(!app.ui.show_audio_health);
        assert!(app.audio_health_report.is_none());
        app.run("window.audio_health", json!({"value": true})).unwrap();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| audio_health(&mut app, ui.ctx()));
        output.textures_delta.clear();
        assert!(app.audio_health_report.is_some());
    }
}
