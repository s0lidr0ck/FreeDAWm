//! Bounce Mix renders a slice per frame behind a progress window, so the app keeps drawing and the
//! bounce can be cancelled. It renders through the plugin instances that are playing (the player
//! lends them, see [`crate::render`]): what is heard is what is bounced.

use crate::SoundApp;
use crate::theme::{Tokens, bold};
use serde_json::Value;

/// Starts the bounce `file.bounce_mix` runs for `params`; [`step`] renders it.
pub fn start(app: &mut SoundApp, params: &Value) {
    if app.rendering() {
        app.ui.status = "A render is already running".into();
        return;
    }
    if crate::open_progress::refuses(app) {
        return;
    }
    app.stop_play();
    // Third-party plugins keep state the session can't see: store it first.
    app.capture_plugin_states();
    match soundcraft_engine::cmd::bounce_job(&app.engine, params) {
        Ok(job) => {
            app.bounce = Some(job);
            crate::render::lend(app);
            app.ui.status = crate::render::RENDERING.into();
        }
        Err(e) => app.ui.status = e.to_string(),
    }
}

/// Starts Export › Stems (`file.bounce_stems` for `params`) behind a progress window: the
/// transport stops and [`step`] renders it a slice per frame, so the window keeps drawing and
/// Cancel works. It renders through instances of its own (a stem solos one track).
pub fn start_stems(app: &mut SoundApp, params: &Value) -> Result<Value, String> {
    if app.rendering() {
        app.ui.status = "A render is already running".into();
        return Err(app.ui.status.clone());
    }
    if crate::open_progress::refuses(app) {
        return Err(app.ui.status.clone());
    }
    app.stop_play();
    app.capture_plugin_states();
    match soundcraft_engine::cmd::stems_job(&app.engine, params) {
        Ok(job) => {
            let n = job.total();
            app.stems = Some(job);
            app.ui.status = crate::render::RENDERING.into();
            Ok(serde_json::json!({"stems": n, "rendering": true}))
        }
        Err(e) => {
            app.ui.status = e.to_string();
            Err(app.ui.status.clone())
        }
    }
}

/// Renders the running stems export for a moment (see [`step`]).
fn step_stems(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(job) = &mut app.stems else { return };
    app.ui.status = match job.step(slice()) {
        Ok(None) => {
            ctx.request_repaint();
            return;
        }
        Ok(Some(files)) => format!("Exported {} stem(s)", files.len()),
        Err(e) => e.to_string(),
    };
    app.stems = None;
}

/// Starts the movie `file.bounce_movie` writes for `params`; [`step`] writes it a few frames at a
/// time, the status line showing how far it is.
#[cfg(not(target_arch = "wasm32"))]
pub fn start_movie(app: &mut SoundApp, params: &Value) {
    if app.rendering() {
        app.ui.status = "A render is already running".into();
        return;
    }
    if crate::open_progress::refuses(app) {
        return;
    }
    app.stop_play();
    app.capture_plugin_states();
    match soundcraft_engine::cmd::movie_bounce::movie_job(&app.engine, params) {
        Ok(job) => {
            app.movie = Some(job);
            app.ui.status = "Bouncing the movie…".into();
        }
        Err(e) => app.ui.status = e.to_string(),
    }
}

/// Writes the running movie bounce for a moment (see [`start_movie`]).
#[cfg(not(target_arch = "wasm32"))]
fn step_movie(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(job) = &mut app.movie else { return };
    app.ui.status = match job.step(slice()) {
        Ok(None) => {
            app.ui.status = format!("Bouncing the movie… {:.0} %", job.progress() * 100.0);
            ctx.request_repaint();
            return;
        }
        Ok(Some(done)) => {
            let name = job.path().file_name().unwrap_or_default().to_string_lossy().into_owned();
            let mut line = format!("Bounced {name} ({} frames, {}×{}, {:.3} fps)", done.frames, done.width, done.height, done.fps);
            if done.black_frames > 0 {
                line.push_str(&format!("; {} black where no Video clip plays", done.black_frames));
            }
            line
        }
        Err(e) => e.to_string(),
    };
    app.movie = None;
}

/// Renders the running bounce for a moment. Called from `logic`, which eframe keeps calling while
/// the window is hidden as long as a repaint is requested, so a bounce doesn't pause then.
pub fn step(app: &mut SoundApp, ctx: &egui::Context) {
    step_stems(app, ctx);
    #[cfg(not(target_arch = "wasm32"))]
    step_movie(app, ctx);
    if app.bounce.is_none() || !crate::render::instances_ready(app, ctx) {
        return;
    }
    let Some(job) = &mut app.bounce else { return };
    let mut import = None;
    app.ui.status = match job.step(slice()) {
        Ok(None) => {
            ctx.request_repaint();
            return;
        }
        Ok(Some(done)) => {
            let name = job.path().file_name().unwrap_or_default().to_string_lossy().into_owned();
            if job.imports() {
                import = Some((done.files.iter().map(|f| f.to_string_lossy().into_owned()).collect::<Vec<_>>(), job.range().start));
            }
            done_line(&name, &done)
        }
        Err(e) => e.to_string(),
    };
    app.bounce = None;
    // Add to Session: the files go onto new tracks at the range's start, as one undo step.
    if let Some((files, at)) = import {
        let main_ext = files.first().and_then(|f| std::path::Path::new(f).extension().map(|x| x.to_os_string()));
        let paths: Vec<&String> =
            files.iter().filter(|f| std::path::Path::new(f.as_str()).extension().map(|x| x.to_os_string()) == main_ext).collect();
        if let Err(e) = app.run("file.import_audio", serde_json::json!({"paths": paths, "at": at, "new_tracks": true})) {
            app.ui.status = e;
        }
    }
}

/// The status line for a finished bounce: its levels, and for a loudness target held back by the
/// true-peak ceiling, by how much.
pub fn done_line(name: &str, done: &soundcraft_engine::io::Bounced) -> String {
    let mut line = format!("Bounced {name} (peak {:.1} dBFS, {:.1} LUFS, true peak {:.1} dBTP)", done.peak_db, done.lufs, done.true_peak_db);
    if let Some(m) = done.measured.filter(|m| m.held_by_ceiling) {
        line.push_str(&format!(
            ". The true-peak ceiling held the gain at {:+.1} dB, short of the loudness target: a limiter on the master would let it get there",
            m.gain_db
        ));
    }
    line
}

/// Refuses to replace the session while a render runs (it holds the player's instances, and a
/// bounce its own copy of the session): for the paths that bypass [`SoundApp::run`] (a session
/// dropped on the window); session commands are refused by `render::blocked`. True when
/// refused; the status line says why, as `run` does.
pub fn refuses_session_switch(app: &mut SoundApp) -> bool {
    if app.rendering() {
        app.ui.status = crate::render::RENDERING.into();
        return true;
    }
    crate::open_progress::refuses(app)
}

/// Whether to render another block in this call: for 30 ms.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn slice() -> impl FnMut() -> bool {
    let until = std::time::Instant::now() + std::time::Duration::from_millis(30);
    move || std::time::Instant::now() < until
}

/// The browser has no `Instant` (it panics there), so a call renders 32 blocks.
#[cfg(target_arch = "wasm32")]
pub(crate) fn slice() -> impl FnMut() -> bool {
    let mut blocks = 32;
    move || {
        blocks -= 1;
        blocks > 0
    }
}

/// Shows how far the running stems export got, with Cancel (or Esc).
fn show_stems(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(job) = &app.stems else { return };
    let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    egui::Modal::new(egui::Id::new("stems_progress")).show(ctx, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        ui.set_width(320.0);
        let name = job.current().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        ui.label(egui::RichText::new(format!("Exporting {} stems", job.total())).font(bold(13.0)));
        ui.label(egui::RichText::new(name).color(Tokens::current().text_dim));
        ui.add(egui::ProgressBar::new(job.progress()));
        ui.horizontal(|ui| {
            cancel |= ui.button("Cancel").clicked();
            ui.label(egui::RichText::new(format!("{:.0}%", job.progress() * 100.0)).color(Tokens::current().text_dim));
        });
    });
    if cancel {
        app.stems = None;
        app.ui.status = "Stems export cancelled".into();
    }
}

/// Shows how far the running bounce got, with Cancel (or Esc).
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    show_stems(app, ctx);
    let Some(job) = &app.bounce else { return };
    let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    egui::Modal::new(egui::Id::new("bounce_progress")).show(ctx, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        ui.set_width(320.0);
        let name = job.path().file_name().unwrap_or_default().to_string_lossy();
        ui.label(egui::RichText::new(format!("Bouncing {name}")).font(bold(13.0)));
        ui.add(egui::ProgressBar::new(job.progress()));
        ui.horizontal(|ui| {
            cancel |= ui.button("Cancel").clicked();
            let through = if job.is_live() { " through the live plugins" } else { "" };
            ui.label(egui::RichText::new(format!("{:.0}%{through}", job.progress() * 100.0)).color(Tokens::current().text_dim));
        });
    });
    if cancel {
        // (dropping the bounce hands the lent instances back)
        app.bounce = None;
        app.lending = None;
        app.ui.status = "Bounce cancelled".into();
    }
}
