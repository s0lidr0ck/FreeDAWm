//! Track › Capture to Pantry…: the notes are rendered a slice per frame behind a progress window
//! (a piano's 360 samples are most of an hour of audio; the UI must not freeze), then
//! `track.capture_multisample` runs with the capture ready
//! ([`soundcraft_engine::cmd::multisample::CaptureJob`]). Cancel (or Esc) leaves the session
//! untouched. A session opened, made or closed meanwhile drops the job.

use crate::SoundApp;
use crate::theme::{Tokens, bold};
use serde_json::{Value, json};
use soundcraft_engine::cmd::multisample::CaptureJob;

/// The command the job runs for.
pub const ID: &str = "track.capture_multisample";

/// The job running, and the session it was planned in (`Engine::loads`).
pub struct Running {
    job: CaptureJob,
    loads: u64,
}

/// Starts the capture for `track.capture_multisample` with `params`. The result is what the
/// caller returns for the command.
pub fn start(app: &mut SoundApp, params: &Value) -> Result<Value, String> {
    if app.capture.is_some() {
        app.ui.status = "Capturing…".into();
        return Err(app.ui.status.clone());
    }
    // Third-party plugins keep state the session can't see: the job renders through instances
    // of its own, which read it.
    app.capture_plugin_states();
    app.prepare_ara_renders();
    match CaptureJob::new(&app.engine, params) {
        Ok(job) => {
            app.capture = Some(Running { job, loads: app.engine.loads() });
            app.ui.status = "Capturing…".into();
            Ok(json!({"capturing": true}))
        }
        Err(e) => {
            app.ui.status = e.to_string();
            Err(e.to_string())
        }
    }
}

/// Renders for a moment; once done, runs the command. Called from `logic` every frame.
pub fn step(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(run) = &mut app.capture else { return };
    if run.loads != app.engine.loads() {
        app.capture = None;
        return;
    }
    match run.job.step(crate::bounce::slice()) {
        Ok(false) => {
            ctx.request_repaint();
            return;
        }
        Ok(true) => {}
        Err(e) => {
            app.capture = None;
            app.ui.status = e.to_string();
            return;
        }
    }
    let Some(run) = app.capture.take() else { return };
    let params = run.job.command().1.clone();
    match run.job.finish() {
        Ok(made) => app.engine.offer_capture(made),
        Err(e) => {
            app.ui.status = e.to_string();
            return;
        }
    }
    // The status line is set where every run of the command sets it (`SoundApp::run_command`).
    if let Err(e) = app.run_command(ID, params) {
        app.ui.status = e;
    }
}

/// Shows how far the capture got, with Cancel (or Esc).
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(run) = &app.capture else { return };
    let (done, total) = run.job.counts();
    let progress = run.job.progress();
    let note = run.job.note();
    let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    egui::Modal::new(egui::Id::new("capture_progress")).show(ctx, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        ui.set_width(320.0);
        ui.label(egui::RichText::new("Capturing to Pantry").font(bold(13.0)));
        ui.add(egui::ProgressBar::new(progress));
        ui.horizontal(|ui| {
            cancel |= ui.button("Cancel").clicked();
            let text = match note {
                Some(n) => format!("{n} · note {} of {total}", done + 1),
                None => format!("{total} notes"),
            };
            ui.label(egui::RichText::new(text).color(Tokens::current().text_dim));
        });
    });
    if cancel {
        app.capture = None;
        app.ui.status = "Capture to Pantry cancelled".into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session with one instrument track playing the built-in synth.
    fn app() -> (SoundApp, u64) {
        let mut e = soundcraft_engine::Engine::default();
        let t = e.execute("track.new", &json!({"kind": "instrument", "name": "Keys"})).unwrap()["tracks"][0].as_u64().unwrap();
        (SoundApp::new(e, None, crate::Services::default()), t)
    }

    fn params(t: u64) -> Value {
        json!({"track": t, "low": 60, "high": 63, "step": 3, "layers": 2, "hold": 0.2, "tail": 0.4})
    }

    #[test]
    fn the_app_captures_behind_a_progress_window_in_one_undo_step() {
        let (mut app, t) = app();
        let ctx = egui::Context::default();
        assert_eq!(app.run(ID, params(t)).unwrap(), json!({"capturing": true}));
        assert!(app.capture.is_some());
        assert!(app.run(ID, params(t)).is_err(), "one capture at a time");
        while app.capture.is_some() {
            assert_eq!(app.engine.session().tracks.len(), 1, "nothing changes until the capture is done");
            step(&mut app, &ctx);
        }
        assert_eq!(app.ui.status, "Captured 4 samples of 2 notes");
        assert_eq!(app.engine.session().tracks.len(), 2);
        assert!(app.engine.undo());
        assert_eq!(app.engine.session().tracks.len(), 1);
    }

    #[test]
    fn programmatic_calls_capture_at_once_and_cancel_leaves_the_session_alone() {
        let (mut app, t) = app();
        assert_eq!(app.run_sync(ID, params(t)).unwrap()["samples"], 4);
        let (mut app2, t2) = self::app();
        app2.run(ID, params(t2)).unwrap();
        app2.capture = None;
        assert_eq!(app2.engine.session().tracks.len(), 1);
        // A capture that can't start says why and starts nothing.
        let mut bad = params(t2);
        bad["low"] = json!(90);
        assert!(app2.run(ID, bad).is_err() && app2.capture.is_none());
    }
}
