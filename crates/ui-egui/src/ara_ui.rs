//! Clip › ARA Plug-ins… (`window.ara`): give the selected audio clips to an ARA plug-in the VST3
//! scan found (`clip.ara_add`), take them back (`clip.ara_remove`), and see what each plug-in
//! has done with them. The session's ARA documents live in [`SoundApp::ara`] and follow the
//! session every frame ([`tick`]), on this (the UI) thread, which is ARA's model thread.

use crate::SoundApp;
use serde_json::json;

/// Keeps the ARA documents in line with the session (cheap when no clip has an ARA plug-in).
pub fn tick(app: &mut SoundApp) {
    app.ara.sync(&app.engine);
}

/// Why Clip › ARA Plug-ins… is greyed out, if it is.
pub fn availability() -> Result<(), String> {
    if soundcraft_vst3_host::ara_plugins().is_empty() {
        Err("No ARA plug-in found: install an ARA 2 VST3 plug-in, then scan for plug-ins".into())
    } else {
        Ok(())
    }
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    if !app.ui.show_ara {
        return;
    }
    let mut open = true;
    let plugins = soundcraft_vst3_host::ara_plugins();
    if app.ara_pick.as_ref().is_none_or(|p| !plugins.iter().any(|d| &d.id == p)) {
        app.ara_pick = plugins.first().map(|d| d.id.clone());
    }
    let mut run: Option<(&'static str, serde_json::Value)> = None;
    egui::Window::new("ARA Plug-ins").open(&mut open).default_size(egui::vec2(560.0, 420.0)).resizable(true).show(ctx, |ui| {
        ui.label("An ARA plug-in reads a clip's audio straight from the session and follows its edits.");
        ui.label(
            egui::RichText::new(
                "Stage one: the plug-in reads and analyses the clips' audio. The clips still play their own audio; \
                 playback through the plug-in, saving its edits and its editor window are not built yet.",
            )
            .weak(),
        );
        ui.separator();
        if plugins.is_empty() {
            ui.label("No ARA plug-in found. Install an ARA 2 VST3 plug-in, then scan for plug-ins.");
        }
        for d in &plugins {
            let Some(a) = &d.ara else { continue };
            let picked = app.ara_pick.as_deref() == Some(d.id.as_str());
            let title = format!("{} — {} {}", d.name, a.manufacturer, a.version);
            if ui.radio(picked, title).clicked() {
                app.ara_pick = Some(d.id.clone());
            }
            ui.label(
                egui::RichText::new(format!(
                    "    roles: {} · analyses: {} · ARA generations {} to {}",
                    if a.roles.is_empty() { "none".into() } else { a.roles.join(", ") },
                    if a.content.is_empty() { "nothing it says".into() } else { a.content.join(", ") },
                    a.lowest_generation,
                    a.highest_generation
                ))
                .weak(),
            );
        }
        ui.separator();
        let selected: Vec<_> = app.engine.session().edit.selected_clips.clone();
        let audio: Vec<_> =
            selected.iter().filter_map(|id| app.engine.session().find_clip(*id).map(|(_, c)| c.clone())).filter(|c| c.is_audio()).collect();
        ui.label(format!("{} audio clip(s) selected", audio.len()));
        ui.horizontal(|ui| {
            let can_add = app.ara_pick.is_some() && !audio.is_empty();
            let add = ui.add_enabled(can_add, egui::Button::new("Give Selected Clips to the Plug-in"));
            let add = if can_add { add } else { add.on_disabled_hover_text("Select audio clips and an ARA plug-in") };
            if add.clicked()
                && let Some(p) = &app.ara_pick
            {
                run = Some(("clip.ara_add", json!({"plugin": p})));
            }
            let any = audio.iter().any(|c| c.ara.is_some());
            let take = ui.add_enabled(any, egui::Button::new("Take Them Back"));
            let take = if any { take } else { take.on_disabled_hover_text("Select a clip given to an ARA plug-in") };
            if take.clicked() {
                run = Some(("clip.ara_remove", json!({})));
            }
        });
        egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
            for c in &audio {
                let line = app.ara.status(c).unwrap_or_else(|| "no ARA plug-in".into());
                ui.label(format!("{}: {line}", c.name));
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
    if let Some((id, p)) = run {
        let _ = app.run(id, p);
    }
    app.ui.show_ara = open;
}
