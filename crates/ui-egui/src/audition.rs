//! Auditioning audio that is not playing on the timeline: an audio file in the Workspace browser
//! (decoded on a worker thread, so a long file does not hold up the window) and a clip in the
//! Clip List, through the player's preview voice over the mix, at the preview level
//! (`UiState::preview_db`). Clicking what is playing again stops it.

use crate::SoundApp;
use soundcraft_model::{ClipContent, ClipId};
use std::sync::mpsc::{Receiver, channel};

/// The longest a preview plays, in seconds (a file is decoded whole, then cut).
pub const MAX_SECONDS: f64 = 120.0;

/// Audition state on the app.
#[derive(Default)]
pub struct Audition {
    /// A file being decoded: its path and the worker's answer.
    pending: Option<(String, Receiver<Result<(Vec<Vec<f32>>, u32), String>>)>,
    /// What is playing (a file's path, or `clip:<id>`), until another or Stop.
    pub playing: Option<String>,
}

/// The planar audio at `rate` cut to [`MAX_SECONDS`] and set to the preview level.
fn prepared(mut channels: Vec<Vec<f32>>, rate: u32, db: f32) -> Vec<Vec<f32>> {
    let max = (MAX_SECONDS * f64::from(rate.max(1))) as usize;
    let g = if db.is_finite() { 10f32.powf(db.clamp(-60.0, 12.0) / 20.0) } else { 1.0 };
    for ch in &mut channels {
        ch.truncate(max);
        ch.iter_mut().for_each(|x| *x *= g);
    }
    channels
}

fn play(app: &mut SoundApp, what: String, channels: Vec<Vec<f32>>, rate: u32) {
    let channels = prepared(channels, rate, app.ui.preview_db);
    if let Some(p) = &app.player {
        p.preview(channels, rate);
    }
    app.audition.playing = Some(what);
}

/// Stops what is auditioning.
pub fn stop(app: &mut SoundApp) {
    if app.audition.playing.take().is_some()
        && let Some(p) = &app.player
    {
        p.preview(Vec::new(), 48_000);
    }
    app.audition.pending = None;
}

/// Auditions the audio file at `path`, or stops it when it is what plays.
pub fn file(app: &mut SoundApp, path: &str) {
    if app.audition.playing.as_deref() == Some(path) || app.audition.pending.as_ref().is_some_and(|(p, _)| p == path) {
        stop(app);
        return;
    }
    stop(app);
    let (tx, rx) = channel();
    let owned = path.to_string();
    let spawned = std::thread::Builder::new().name("audition".into()).spawn(move || {
        let ext = std::path::Path::new(&owned).extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
        let got = std::fs::read(&owned)
            .map_err(|e| e.to_string())
            .and_then(|bytes| soundcraft_audio_io::decode(&bytes, ext.as_deref()).map_err(|e| e.to_string()))
            .map(|(_, buf)| (buf.channels, buf.sample_rate));
        let _ = tx.send(got);
    });
    match spawned {
        Ok(_) => app.audition.pending = Some((path.to_string(), rx)),
        Err(e) => app.ui.status = format!("Cannot audition: {e}"),
    }
}

/// Auditions `clip` as it sounds (its material with clip gain), or stops it when it is what plays.
pub fn clip(app: &mut SoundApp, id: ClipId) {
    let what = format!("clip:{}", id.0);
    if app.audition.playing.as_deref() == Some(what.as_str()) {
        stop(app);
        return;
    }
    stop(app);
    let s = app.engine.session_arc();
    let Some((_, c)) = s.find_clip(id) else { return };
    let ClipContent::Audio { offset, .. } = c.content else {
        app.ui.status = "Only audio clips can be auditioned".into();
        return;
    };
    let rate = s.sample_rate.hz();
    let n = usize::try_from(c.source_span(c.length)).unwrap_or(0).min((MAX_SECONDS * f64::from(rate)) as usize);
    let Some(mut channels) = soundcraft_mix::clip_source_window(&s, c, offset, n) else { return };
    for ch in &mut channels {
        for (k, x) in ch.iter_mut().enumerate() {
            *x *= c.gain_at(c.timeline_span(k as i64));
        }
    }
    play(app, what, channels, rate);
}

/// Starts a decoded file once it is ready (each frame), and forgets a preview that ended.
pub fn poll(app: &mut SoundApp) {
    let Some((path, rx)) = &app.audition.pending else { return };
    let path = path.clone();
    match rx.try_recv() {
        Ok(Ok((channels, rate))) => {
            app.audition.pending = None;
            play(app, path, channels, rate);
        }
        Ok(Err(e)) => {
            app.audition.pending = None;
            app.ui.status = format!("Cannot audition {path}: {e}");
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {}
        Err(std::sync::mpsc::TryRecvError::Disconnected) => app.audition.pending = None,
    }
}

/// Whether a file is still being read for audition.
pub fn loading(app: &SoundApp) -> bool {
    app.audition.pending.is_some()
}

#[cfg(test)]
mod tests {
    #[test]
    fn previews_are_cut_and_set_to_the_preview_level() {
        let out = super::prepared(vec![vec![1.0; 300]], 1, -6.0);
        assert_eq!(out[0].len(), 120);
        assert!((out[0][0] - 0.501).abs() < 0.01);
    }
}
