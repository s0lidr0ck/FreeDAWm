//! Edit › Split into Files from the menu: the pieces are copied a few per frame behind a
//! progress window (an hour of 16+ tracks must not freeze the UI), then `edit.split_into_files`
//! runs with the copies ready ([`soundcraft_engine::split_files`]). Cancel (or Esc) leaves the
//! session untouched. A session opened, made or closed meanwhile drops the job.

use crate::SoundApp;
use crate::theme::{Tokens, bold};
use serde_json::{Value, json};
use soundcraft_engine::split_files::{ID, SplitJob};

/// The job running, and the session it was planned in (`Engine::loads`).
pub struct Running {
    job: SplitJob,
    loads: u64,
}

/// Starts the copies for `edit.split_into_files` with `params`. `None` when there is nothing to
/// copy (only cuts): run the command as it is. The result is what the caller returns.
pub fn start(app: &mut SoundApp, params: &Value) -> Option<Result<Value, String>> {
    if app.split.is_some() {
        app.ui.status = "Splitting into files…".into();
        return Some(Err(app.ui.status.clone()));
    }
    match SplitJob::new(&app.engine, params) {
        Ok(Some(job)) => {
            app.split = Some(Running { job, loads: app.engine.loads() });
            app.ui.status = "Splitting into files…".into();
            Some(Ok(json!({"splitting": true})))
        }
        Ok(None) => None,
        Err(e) => {
            app.ui.status = e.to_string();
            Some(Err(e.to_string()))
        }
    }
}

/// Copies for a moment; once done, runs the command. Called from `logic` every frame.
pub fn step(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(run) = &mut app.split else { return };
    if run.loads != app.engine.loads() {
        app.split = None;
        return;
    }
    if !run.job.step(crate::bounce::slice()) {
        ctx.request_repaint();
        return;
    }
    let Some(run) = app.split.take() else { return };
    let params = run.job.command().1.clone();
    run.job.finish(&mut app.engine);
    match app.run_command(ID, params) {
        Ok(v) => app.ui.status = status(&v),
        Err(e) => app.ui.status = e,
    }
}

/// The status line after the command returned `v`.
pub fn status(v: &Value) -> String {
    let files = v.get("files").and_then(Value::as_u64).unwrap_or(0);
    let songs = v.get("segments").and_then(Value::as_array).map_or(0, Vec::len);
    let kept = v.get("kept").and_then(Value::as_array).map_or(0, Vec::len);
    let mut s = format!("Split into {songs} part{}: {files} new file{}", if songs == 1 { "" } else { "s" }, if files == 1 { "" } else { "s" });
    if kept > 0 {
        s.push_str(&format!(
            " ({kept} clip{} kept {} file: stretched, cleaned or tuned)",
            if kept == 1 { "" } else { "s" },
            if kept == 1 { "its" } else { "their" }
        ));
    }
    s
}

/// Shows how far the copies got, with Cancel (or Esc).
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(run) = &app.split else { return };
    let (done, total) = run.job.counts();
    let progress = run.job.progress();
    let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    egui::Modal::new(egui::Id::new("split_progress")).show(ctx, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        ui.set_width(320.0);
        ui.label(egui::RichText::new("Splitting into Files").font(bold(13.0)));
        ui.add(egui::ProgressBar::new(progress));
        ui.horizontal(|ui| {
            cancel |= ui.button("Cancel").clicked();
            ui.label(egui::RichText::new(format!("{done} of {total} files")).color(Tokens::current().text_dim));
        });
    });
    if cancel {
        app.split = None;
        app.ui.status = "Split into Files cancelled".into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_audio_io::{AudioBuffer, FileFormat};
    use soundcraft_model::{ChannelFormat, Clip, MarkerKind, Session, TrackKind};

    /// One 3 s track with a marker in the middle.
    fn app() -> SoundApp {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
        let src = soundcraft_engine::io::add_source(
            &mut s,
            "Vox_01",
            AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 144_000]] },
            None,
            FileFormat::Wav,
        );
        let c = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(c, "Vox_01", src, 0, 0, 144_000));
        s.add_marker("Song", MarkerKind::Marker, 72_000, 72_000);
        SoundApp::new(soundcraft_engine::Engine::new(s), None, crate::Services::default())
    }

    #[test]
    fn the_menu_splits_behind_a_progress_window_in_one_undo_step() {
        let mut app = app();
        let ctx = egui::Context::default();
        assert_eq!(app.run(ID, json!({})).unwrap(), json!({"splitting": true}));
        assert!(app.split.is_some());
        assert_eq!(app.engine.session().tracks[0].clips().len(), 1, "nothing changes until the copies are done");
        while app.split.is_some() {
            step(&mut app, &ctx);
        }
        assert_eq!(app.ui.status, "Split into 2 parts: 2 new files");
        let names: Vec<String> = app.engine.session().tracks[0].clips().iter().map(|c| c.name.clone()).collect();
        assert_eq!(names, ["Vox - Part 1", "Vox - Song"]);
        assert!(app.engine.undo());
        assert_eq!(app.engine.session().tracks[0].clips().len(), 1);
    }

    #[test]
    fn programmatic_calls_split_at_once_and_cancel_leaves_the_session_alone() {
        let mut app = app();
        assert_eq!(app.run_sync(ID, json!({})).unwrap()["files"], 2);
        let mut app2 = self::app();
        app2.run(ID, json!({})).unwrap();
        app2.split = None;
        assert_eq!(app2.engine.session().tracks[0].clips().len(), 1);
    }
}
