//! Clip › ARA Plug-ins… (`window.ara`) and the ARA submenu of an audio clip's context menu: give
//! audio clips to an ARA plug-in the VST3 scan found (`clip.ara_add`), bypass it
//! (`clip.ara_bypass`), take them back (`clip.ara_remove`), and see what each plug-in has done
//! with them. The session's ARA documents live in [`SoundApp::ara`] and follow the session every
//! frame ([`tick`]), on this (the UI) thread, which is ARA's model thread.

use crate::SoundApp;
use serde_json::json;
use soundcraft_model::ClipId;
use std::sync::atomic::{AtomicBool, Ordering};

/// Keeps the ARA documents in line with the session (cheap when no clip has an ARA plug-in).
pub fn tick(app: &mut SoundApp) {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if !LOGGED.swap(true, Ordering::Relaxed) {
        log_scan();
    }
    // Clips play through their plug-in's voices on the playing engine (prepared at the rate the
    // mix renders: the session's).
    match &app.player {
        Some(p) => app.ara.set_realtime(Some(soundcraft_playback::ara_board()), app.engine.session().sample_rate.hz() as f32, p.max_block()),
        None => app.ara.set_realtime(None, 0.0, 0),
    }
    app.ara.sync(&app.engine);
    if app.ara.take_unsaved() {
        app.engine.mark_unsaved();
    }
    for m in app.ara.take_messages() {
        app.ui.status = m;
    }
    for r in app.ara.take_requests() {
        playback_request(app, r);
    }
}

/// Carries out what an ARA plug-in asked of the transport (its playback controller): play, stop,
/// locate, the loop range (the edit selection on the clip's tracks, as Loop Playback plays it)
/// and looping on or off.
fn playback_request(app: &mut SoundApp, r: soundcraft_engine::ara::PlaybackRequest) {
    use soundcraft_engine::ara::PlaybackRequest as R;
    let hz = f64::from(app.engine.session().sample_rate.hz());
    let at = |t: f64| (t.max(0.0) * hz).round() as i64;
    let done = match r {
        R::Start => app.run("transport.play", json!({})),
        R::Stop => app.run("transport.stop", json!({})),
        R::Position(t) => app.run("transport.locate", json!({"at": at(t)})),
        R::Cycle(start, duration) => app.run("edit.select", json!({"start": at(start), "end": at(start + duration)})),
        R::EnableCycle(on) => app.run("options.loop_playback", json!({"value": on})),
    };
    if let Err(e) = done {
        soundcraft_engine::ara::log_line(&format!("a plug-in's request to {} was not carried out: {e}", r.describe()));
    }
}

/// Writes to the ARA log, for every VST3 module the scan knows, whether it offers ARA 2 and why
/// not (at start-up and after every plug-in scan), so a plug-in missing from the list can be
/// diagnosed from the log alone.
pub fn log_scan() {
    let report = soundcraft_vst3_host::ara_scan_report();
    let found = soundcraft_vst3_host::ara_plugins().len();
    soundcraft_engine::ara::log_line(&format!("plug-in scan: {} VST3 module(s) or file(s) known, {found} offer usable ARA 2", report.len()));
    for dir in soundcraft_vst3_host::search_dirs() {
        soundcraft_engine::ara::log_line(&format!("VST3 folder scanned: {}", dir.display()));
    }
    for line in report {
        soundcraft_engine::ara::log_line(&line);
    }
}

/// `ui.ara_editor {clip?}`: opens the editor window of the clip's ARA plug-in (the first selected
/// clip given to one when no clip is named), told the Edit window's selection.
pub fn open_editor(app: &mut SoundApp, p: &serde_json::Value) -> Result<serde_json::Value, String> {
    let s = app.engine.session();
    let clip = p
        .get("clip")
        .and_then(serde_json::Value::as_u64)
        .map(ClipId)
        .or_else(|| s.edit.selected_clips.iter().copied().find(|c| s.find_clip(*c).is_some_and(|(_, x)| x.ara.is_some())))
        .ok_or("Select a clip given to an ARA plug-in")?;
    app.ara.sync(&app.engine);
    let s = app.engine.session_arc();
    app.ara.open_editor(&s, clip)?;
    Ok(json!({"clip": clip.0}))
}

/// Why Clip › ARA Plug-ins… is greyed out, if it is.
pub fn availability() -> Result<(), String> {
    if soundcraft_vst3_host::ara_plugins().is_empty() {
        Err("No ARA plug-in found: install an ARA 2 VST3 plug-in, then scan for plug-ins".into())
    } else {
        Ok(())
    }
}

/// An ARA plug-in the scan found.
struct Found {
    id: String,
    name: String,
    /// "maker version".
    maker: String,
    /// Roles, what it analyses, generations, where it is.
    details: String,
}

fn found() -> Vec<Found> {
    soundcraft_vst3_host::ara_plugins()
        .into_iter()
        .filter_map(|d| {
            let a = d.ara?;
            let name = if a.name.is_empty() { d.name.clone() } else { a.name.clone() };
            let details = format!(
                "Roles: {}\nAnalyses: {}\nARA API generations {} to {}\n{}",
                if a.roles.is_empty() { "none".into() } else { a.roles.join(", ") },
                if a.content.is_empty() { "nothing it says".into() } else { a.content.join(", ") },
                a.lowest_generation,
                a.highest_generation,
                d.path.display()
            );
            Some(Found { id: d.id, name, maker: format!("{} {}", a.manufacturer, a.version).trim().to_string(), details })
        })
        .collect()
}

/// The clips a clip-menu action on `c` applies to: the selected clips when `c` is among them,
/// else `c` alone.
fn menu_clips(app: &SoundApp, c: ClipId) -> Vec<u64> {
    let sel = &app.engine.session().edit.selected_clips;
    if sel.contains(&c) { sel.iter().map(|x| x.0).collect() } else { vec![c.0] }
}

/// The ARA submenu of an audio clip's context menu (MIDI clips get none): "Edit with <plug-in>"
/// for each ARA plug-in found; on a clip already given to one, Open Editor, Bypass and Remove;
/// and ARA Plug-ins… for the window. Greyed with the reason (and a Rescan item) when no ARA
/// plug-in was found.
pub(crate) fn clip_menu(app: &mut SoundApp, ui: &mut egui::Ui, c: ClipId) {
    let Some(clip) = app.engine.session().find_clip(c).map(|(_, x)| x.clone()).filter(|x| x.is_audio()) else { return };
    let plugins = found();
    ui.separator();
    if plugins.is_empty() && clip.ara.is_none() {
        let why = "No ARA plug-in found: install an ARA 2 VST3 plug-in, then rescan plugins";
        ui.add_enabled(false, egui::Button::new("ARA")).on_disabled_hover_text(why);
        if ui.button("Rescan plugins").on_hover_text("Scan new and changed plug-ins, ARA ones included").clicked() {
            crate::browser::scan_job::request(false);
            ui.close();
        }
        return;
    }
    ui.menu_button("ARA", |ui| {
        let clips = menu_clips(app, c);
        if let Some(a) = &clip.ara {
            let name = if a.name.is_empty() { a.plugin.clone() } else { a.name.clone() };
            if let Some(line) = app.ara.status(&clip) {
                ui.label(egui::RichText::new(line).small().weak());
            }
            let editor = app.ara.can_open_editor(&clip);
            let open = ui.add_enabled(editor.is_ok(), egui::Button::new(format!("Open {name} Editor")));
            let open = match &editor {
                Ok(()) => open,
                Err(why) => open.on_disabled_hover_text(why),
            };
            if open.clicked() {
                if let Err(e) = open_editor(app, &json!({"clip": c.0})) {
                    app.ui.status = e;
                }
                ui.close();
            }
            let mut bypass = a.bypass;
            if ui.checkbox(&mut bypass, format!("Bypass {name}")).on_hover_text("The clip plays its own audio; the plug-in keeps its edits").changed()
            {
                let _ = app.run("clip.ara_bypass", json!({"clips": clips, "on": bypass}));
                ui.close();
            }
            if ui.button(format!("Remove {name}")).on_hover_text("Take the clip back from the plug-in").clicked() {
                let _ = app.run("clip.ara_remove", json!({"clips": clips}));
                ui.close();
            }
            ui.separator();
        }
        for p in &plugins {
            if clip.ara.as_ref().is_some_and(|a| a.plugin == p.id) {
                continue;
            }
            let item = ui.button(format!("Edit with {}", p.name)).on_hover_text(format!("{}\n{}", p.maker, p.details));
            if item.clicked() {
                match app.run("clip.ara_add", json!({"clips": clips, "plugin": p.id})) {
                    Ok(_) => {
                        app.ara.sync(&app.engine);
                        let given = app.engine.session().find_clip(c).map(|(_, x)| x.clone());
                        if given.is_some_and(|x| app.ara.can_open_editor(&x).is_ok())
                            && let Err(e) = open_editor(app, &json!({"clip": c.0}))
                        {
                            app.ui.status = e;
                        }
                    }
                    Err(e) => app.ui.status = e,
                }
                ui.close();
            }
        }
        ui.separator();
        if ui.button("ARA Plug-ins...").clicked() {
            app.ui.show_ara = true;
            ui.close();
        }
    });
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    if !app.ui.show_ara {
        return;
    }
    let mut open = true;
    let plugins = found();
    if app.ara_pick.as_ref().is_none_or(|p| !plugins.iter().any(|d| &d.id == p)) {
        app.ara_pick = plugins.first().map(|d| d.id.clone());
    }
    let mut run: Option<(&'static str, serde_json::Value)> = None;
    egui::Window::new("ARA Plug-ins").open(&mut open).default_size(egui::vec2(560.0, 460.0)).resizable(true).show(ctx, |ui| {
        ui.label("An ARA plug-in reads a clip's audio straight from the session and follows its edits.");
        ui.separator();
        if plugins.is_empty() {
            ui.label("No ARA plug-in found.");
        } else {
            ui.label(format!("{} ARA plug-in(s) found (hover one for its details):", plugins.len()));
            egui::ScrollArea::vertical().id_salt("ara_found").max_height(150.0).show(ui, |ui| {
                for p in &plugins {
                    let picked = app.ara_pick.as_deref() == Some(p.id.as_str());
                    let r = ui.radio(picked, format!("{} — {}", p.name, p.maker)).on_hover_text(&p.details);
                    if r.clicked() {
                        app.ara_pick = Some(p.id.clone());
                    }
                }
            });
        }
        ui.collapsing("Not listed?", |ui| {
            ui.label(
                "Check the plug-in is installed as a VST3: many ARA plug-ins install their VST3 only when it is chosen in their \
                 installer. FreeDAWm looks for VST3 plug-ins in these folders:",
            );
            for dir in soundcraft_vst3_host::search_dirs() {
                ui.label(egui::RichText::new(format!("  {}", dir.display())).monospace().small());
            }
            ui.label("The ARA log lists every VST3 the scan examined and whether it offers ARA 2.");
            let idle = app.plugin_scan.is_none();
            if ui.add_enabled(idle, egui::Button::new("Rescan")).on_disabled_hover_text("A plug-in scan is running").clicked() {
                crate::browser::scan_job::request(false);
                app.ara.retry();
            }
        });
        ui.separator();
        let selected: Vec<_> = app.engine.session().edit.selected_clips.clone();
        let audio: Vec<_> =
            selected.iter().filter_map(|id| app.engine.session().find_clip(*id).map(|(_, c)| c.clone())).filter(|c| c.is_audio()).collect();
        ui.label(format!("{} audio clip(s) selected", audio.len()));
        let can_add = app.ara_pick.is_some() && !audio.is_empty();
        let add = ui.add_enabled(can_add, egui::Button::new("Give Selected Clips to the Plug-in"));
        let add = if can_add { add } else { add.on_disabled_hover_text("Select audio clips and an ARA plug-in") };
        if add.clicked()
            && let Some(p) = &app.ara_pick
        {
            run = Some(("clip.ara_add", json!({"plugin": p})));
        }
        egui::ScrollArea::vertical().id_salt("ara_clips").max_height(160.0).show(ui, |ui| {
            for c in &audio {
                let line = app.ara.status(c).unwrap_or_else(|| "no ARA plug-in".into());
                ui.label(format!("{}: {line}", c.name));
                let Some(a) = &c.ara else { continue };
                ui.horizontal(|ui| {
                    let editor = app.ara.can_open_editor(c);
                    let b = ui.add_enabled(editor.is_ok(), egui::Button::new("Open Editor"));
                    let b = match &editor {
                        Ok(()) => b,
                        Err(why) => b.on_disabled_hover_text(why),
                    };
                    if b.clicked() {
                        run = Some(("ui.ara_editor", json!({"clip": c.id.0})));
                    }
                    let mut bypass = a.bypass;
                    if ui.checkbox(&mut bypass, "Bypass").on_hover_text("The clip plays its own audio; the plug-in keeps its edits").changed() {
                        run = Some(("clip.ara_bypass", json!({"clips": [c.id.0], "on": bypass})));
                    }
                    if ui.button("Remove").on_hover_text("Take the clip back from the plug-in").clicked() {
                        run = Some(("clip.ara_remove", json!({"clips": [c.id.0]})));
                    }
                });
            }
        });
        let summary = app.ara.summary();
        if !summary.is_empty() {
            ui.separator();
            for line in summary {
                ui.label(line);
            }
        }
        if let Some(path) = soundcraft_engine::ara::log_file() {
            ui.separator();
            ui.label(egui::RichText::new(format!("ARA log: {}", path.display())).weak());
        }
    });
    if let Some((id, p)) = run
        && let Err(e) = app.run(id, p)
    {
        app.ui.status = e;
    }
    app.ui.show_ara = open;
}
