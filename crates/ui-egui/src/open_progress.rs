//! Opening a project shows progress instead of freezing (spec
//! `docs/superpowers/specs/2026-10-28-open-progress.md`). Every command that replaces the session
//! (`session.open`, `session.new`, `session.close`, `session.revert`: Open, Recent, Revert, Close,
//! New, templates, a dropped session file) runs here when invoked from the UI: a modal "Opening
//! <name>…" appears on the next frame over the current session, which stays drawn, inert, and
//! keeps playing until the swap.
//!
//! Phases, in order: Reading session → Loading audio (n of N files) → Waveforms (n of N) →
//! Plugins (n of N, the slow one) → Finishing. The first three run on a worker thread through
//! [`soundcraft_engine::open_job::load_session`] (they touch nothing of the live engine). Hosted
//! plugin instances have main-thread rules, so they are made here, from [`step`], one per frame
//! or several when each takes under [`PLUGIN_BUDGET`] ([`Player::preload_step`]). Finishing is
//! the one synchronous step: the loaded session is installed, and the player takes the instances
//! made ahead with it. Cancel before Finishing discards the new session (instances already made
//! are dropped on the UI thread); the old one stays as it was.
//!
//! While a load runs, every other session-replacing command, Bounce, Freeze, Commit and device
//! changes are refused with [`OPENING`]. The control channel's `session.open` (and the headless
//! hosts') stay synchronous: [`SoundApp::run_sync`].

use crate::SoundApp;
use crate::theme::{Tokens, bold};
use serde_json::{Value, json};
use soundcraft_engine::open_job::{LoadedSession, OpenPhase, OpenProgress, OpenRequest, install_loaded, load_session};
use soundcraft_playback::Player;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex, PoisonError};

/// What the status line says when a command is refused because a project is opening.
pub const OPENING: &str = "Finish or cancel the open first";

/// How long one frame spends making plugin instances: one at least, more while this lasts.
pub const PLUGIN_BUDGET: std::time::Duration = std::time::Duration::from_millis(8);

/// Commands refused while a project is opening: anything that would replace the session again,
/// or render through the player's instances.
pub fn blocked(id: &str) -> bool {
    matches!(
        id,
        "session.new"
            | "session.open"
            | "session.close"
            | "session.revert"
            | "file.import_dawproject"
            | "track.freeze"
            | "track.commit"
            | "track.bounce"
            | "file.bounce_mix"
    )
}

/// Where the open stands.
enum Stage {
    /// The worker reads, decodes and builds waveforms; its progress is read here each frame.
    Loading { rx: Receiver<Result<LoadedSession, String>>, shared: Arc<Mutex<OpenProgress>> },
    /// The session is loaded; the player makes its plugin instances a frame at a time.
    Plugins { loaded: LoadedSession },
    /// Next frame installs the session (Cancel is disabled).
    Finishing { loaded: LoadedSession },
    /// Installed (or cancelled): the job goes.
    Done,
}

/// An open in progress ([`SoundApp::open`]).
pub struct OpenJob {
    /// The command that started it.
    pub id: String,
    /// What the window is titled after: the file's stem or the new session's name.
    pub name: String,
    stage: Stage,
    /// The last progress shown.
    progress: OpenProgress,
    /// `egui` time when it started (elapsed shows against the frame's time).
    started: f64,
    /// Plugins that could not be made (named in the post-open report).
    failed: Vec<String>,
}

impl OpenJob {
    /// The progress shown now (phase, counts, item).
    pub fn progress(&self) -> &OpenProgress {
        &self.progress
    }

    /// The phase under way.
    pub fn phase(&self) -> OpenPhase {
        self.progress.phase
    }

    /// Cancel is possible until Finishing.
    pub fn cancellable(&self) -> bool {
        matches!(self.stage, Stage::Loading { .. } | Stage::Plugins { .. })
    }

    /// Seconds since the open started, at frame time `now`.
    pub fn elapsed(&self, now: f64) -> f64 {
        (now - self.started).max(0.0)
    }
}

/// Starts the open `id` asks for (one of the session-replacing commands) with `params`; the
/// result is what the command returns to its caller at once (`{opening: true}`). Refused with
/// [`OPENING`] while another runs; a bad request (no such file, nothing to revert to) is refused
/// with its reason, in the status line.
pub fn start(app: &mut SoundApp, id: &str, params: &Value) -> Result<Value, String> {
    if refuses(app) {
        return Err(app.ui.status.clone());
    }
    let req = match OpenRequest::from_command(&app.engine, id, params) {
        Some(Ok(req)) => req,
        Some(Err(e)) => {
            app.ui.status = e.to_string();
            log::warn!("{id}: {e}");
            return Err(e.to_string());
        }
        None => return Err(format!("{id}: not a session-replacing command")),
    };
    let name = req.display_name();
    let started = app.last_frame.unwrap_or_default();
    app.ui.status = format!("Opening {name}…");
    if let OpenRequest::Fresh { .. } = &req {
        // Nothing to read (New, Close): straight to the plugins (none) and the install, with no
        // worker, so the swap lands within two frames of the command.
        return match load_session(req, &mut |_| {}) {
            Ok(loaded) => {
                let mut job =
                    OpenJob { id: id.to_string(), name, stage: Stage::Done, progress: OpenProgress::default(), started, failed: Vec::new() };
                job.stage = begin_plugins(app, &mut job, loaded);
                app.open = Some(job);
                Ok(json!({"opening": true}))
            }
            Err(e) => {
                app.ui.status = e.to_string();
                Err(e.to_string())
            }
        };
    }
    let shared = Arc::new(Mutex::new(OpenProgress::new(OpenPhase::Reading, 0, 0, name.clone())));
    let (tx, rx) = channel();
    let work = |req: OpenRequest, shared: Arc<Mutex<OpenProgress>>, tx: std::sync::mpsc::Sender<Result<LoadedSession, String>>| {
        move || {
            let r = load_session(req, &mut |p| *shared.lock().unwrap_or_else(PoisonError::into_inner) = p).map_err(|e| e.to_string());
            let _ = tx.send(r);
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let threaded = std::thread::Builder::new().name("open".into()).spawn(work(req.clone(), Arc::clone(&shared), tx.clone())).is_ok();
        if !threaded {
            log::warn!("the open's worker thread could not start; loading inline");
            work(req, Arc::clone(&shared), tx)();
        }
    }
    // The browser has no threads: the load runs now, and the window shows the plugin phase.
    #[cfg(target_arch = "wasm32")]
    work(req, Arc::clone(&shared), tx)();
    app.open = Some(OpenJob {
        id: id.to_string(),
        name,
        stage: Stage::Loading { rx, shared },
        progress: OpenProgress::new(OpenPhase::Reading, 0, 0, ""),
        started,
        failed: Vec::new(),
    });
    Ok(json!({"opening": true}))
}

/// An open whose loading half is already done (the launch splash loaded the command-line
/// session on its own thread): the job starts at Plugins (or Finishing) and installs `loaded` as
/// [`step`] gets to it. `name` titles the window; `id` is the command it stands for.
pub fn start_loaded(app: &mut SoundApp, id: &str, name: String, loaded: LoadedSession) {
    if app.open.is_some() {
        log::warn!("{id}: an open is already running; the loaded session is dropped");
        return;
    }
    let started = app.last_frame.unwrap_or_default();
    let mut job = OpenJob { id: id.to_string(), name, stage: Stage::Done, progress: OpenProgress::default(), started, failed: Vec::new() };
    job.stage = begin_plugins(app, &mut job, loaded);
    app.open = Some(job);
}

/// True (with [`OPENING`] in the status line) while a project is opening: the caller refuses.
pub fn refuses(app: &mut SoundApp) -> bool {
    if app.open.is_some() {
        app.ui.status = OPENING.into();
        return true;
    }
    false
}

/// Advances the open a little: reads the worker's progress, makes the next plugin instance(s),
/// or installs the session. Called from `logic` every frame; keeps the window repainting.
pub fn step(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(mut job) = app.open.take() else { return };
    job.stage = match std::mem::replace(&mut job.stage, Stage::Done) {
        Stage::Loading { rx, shared } => match rx.try_recv() {
            Ok(Ok(loaded)) => begin_plugins(app, &mut job, loaded),
            Ok(Err(e)) => {
                app.ui.status = e.clone();
                log::warn!("{}: {e}", job.id);
                Stage::Done
            }
            Err(TryRecvError::Empty) => {
                job.progress = shared.lock().unwrap_or_else(PoisonError::into_inner).clone();
                Stage::Loading { rx, shared }
            }
            Err(TryRecvError::Disconnected) => {
                // The worker died (a panic the engine's hook reported): the old session stays.
                app.ui.status = format!("Opening {} failed", job.name);
                log::error!("{}: the loading thread ended without a result", job.id);
                Stage::Done
            }
        },
        Stage::Plugins { loaded } => match app.player.as_ref().and_then(|p| p.preload_step(PLUGIN_BUDGET)) {
            Some(p) if !p.finished() => {
                job.failed = p.failed;
                job.progress = OpenProgress::new(OpenPhase::Plugins, p.done, p.total, p.next.as_deref().map(plugin_name).unwrap_or_default());
                Stage::Plugins { loaded }
            }
            Some(p) => {
                job.failed = p.failed;
                job.progress = OpenProgress::new(OpenPhase::Finishing, 0, 0, "");
                Stage::Finishing { loaded }
            }
            // No player, or the preload is gone: nothing to make.
            None => {
                job.progress = OpenProgress::new(OpenPhase::Finishing, 0, 0, "");
                Stage::Finishing { loaded }
            }
        },
        Stage::Finishing { loaded } => {
            finish(app, &job, loaded);
            Stage::Done
        }
        Stage::Done => Stage::Done,
    };
    if !matches!(job.stage, Stage::Done) {
        app.open = Some(job);
        ctx.request_repaint();
    }
}

/// The session is loaded: the player starts making its hosted instances (Plugins), or, without
/// a player or without hosted plugins, the install is next.
fn begin_plugins(app: &mut SoundApp, job: &mut OpenJob, loaded: LoadedSession) -> Stage {
    let total = app.player.as_ref().map_or(0, |p| p.preload_begin(&loaded.session));
    if total == 0 {
        job.progress = OpenProgress::new(OpenPhase::Finishing, 0, 0, "");
        return Stage::Finishing { loaded };
    }
    let next = app.player.as_ref().and_then(Player::preload_progress).and_then(|p| p.next).as_deref().map(plugin_name).unwrap_or_default();
    job.progress = OpenProgress::new(OpenPhase::Plugins, 0, total, next);
    Stage::Plugins { loaded }
}

/// The one synchronous step: the loaded session replaces the live one, the player takes the
/// instances made ahead with it (`sync_player`, this frame), and the status line reports.
fn finish(app: &mut SoundApp, job: &OpenJob, loaded: LoadedSession) {
    let missing = loaded.missing.len();
    let v = install_loaded(&mut app.engine, loaded);
    for id in &job.failed {
        app.engine.message(format!("plugin not loaded: {}", plugin_name(id)));
    }
    app.after_session_replaced();
    if let Some(note) = crate::sidechain_picker::result_note(&v) {
        log::warn!("{}: {note}", job.id);
        app.ui.status = note;
    } else if job.id == "file.import_dawproject" {
        crate::dawproject_report::offer(app, &v);
    } else if job.id == "session.open" || job.id == "session.revert" {
        let problems = missing.saturating_add(job.failed.len());
        app.ui.status = match (missing, job.failed.len()) {
            (0, 0) => format!("Opened {}", job.name),
            (_, 0) => format!("Opened {} ({problems} missing media)", job.name),
            (0, _) => format!("Opened {} ({problems} plugins not loaded)", job.name),
            _ => format!("Opened {} ({missing} missing media, {} plugins not loaded)", job.name, job.failed.len()),
        };
    } else {
        app.ui.status.clear();
    }
}

/// Discards the open: the loading result is dropped when it comes, the instances made so far are
/// dropped here, and the old session stays exactly as it was. Nothing during Finishing.
pub fn cancel(app: &mut SoundApp) {
    let Some(job) = app.open.take() else { return };
    match job.stage {
        Stage::Loading { .. } => {}
        Stage::Plugins { .. } => {
            if let Some(p) = &app.player {
                p.preload_cancel();
            }
        }
        Stage::Finishing { .. } | Stage::Done => {
            app.open = Some(job);
            return;
        }
    }
    app.ui.status = "Open cancelled".into();
}

/// The name shown for a plugin id: the catalog's, else the id.
fn plugin_name(id: &str) -> String {
    crate::mix_window::plugin_info(id).map_or_else(|| id.to_string(), |p| p.name.to_string())
}

/// The modal: title, bar, phase line, item line, elapsed time, Cancel (or Esc).
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(job) = &app.open else { return };
    let now = ctx.input(|i| i.time);
    let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let dim = Tokens::current().text_dim;
    egui::Modal::new(egui::Id::new("open_progress")).show(ctx, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        ui.set_width(360.0);
        ui.label(egui::RichText::new(format!("Opening {}…", job.name)).font(bold(13.0)));
        ui.add(egui::ProgressBar::new(job.progress.fraction()));
        ui.label(job.progress.phase_line());
        let item = if job.progress.item.is_empty() { " ".to_string() } else { job.progress.item.clone() };
        ui.label(egui::RichText::new(item).color(dim));
        ui.horizontal(|ui| {
            cancel |= ui.add_enabled(job.cancellable(), egui::Button::new("Cancel")).clicked();
            ui.label(egui::RichText::new(format!("{:.1} s", job.elapsed(now))).color(dim));
        });
    });
    if cancel {
        self::cancel(app);
    }
}
