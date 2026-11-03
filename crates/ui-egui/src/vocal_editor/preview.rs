//! The Preview toggle (the header's Preview button, `ui.json` `vocal.preview`, on by default):
//! when a note is clicked, or any of its parameters change (a note drag, the note bar, the side
//! sliders, Snap to key, a macro, Reset, a transition), the clicked or edited note (the first
//! selected when several) plays once from its start to its end through the track's normal path
//! with the current edits, the track soloed for the while (the player is handed a copy of the
//! session with only that track soloed; the document is untouched), and the playhead and edit
//! selection are put back afterwards. A new gesture restarts it. It never starts while the
//! transport is already playing or recording, and it waits (up to [`RENDER_WAIT_SECS`]) for the
//! clip's tuned render to catch up with the edit, so it plays the tuned sound, not the untuned
//! fallback. Tuned/Original is respected as in playback.

use super::layout::Frames;
use super::{editor_track, roll};
use crate::SoundApp;
use soundcraft_engine::vocal::NoteRef;
use soundcraft_model::{Clip, Session, TrackId};
use soundcraft_time::{Range, Samples};
use std::time::Instant;

/// How long a preview waits for the clip's tuned render before playing what there is.
pub const RENDER_WAIT_SECS: f64 = 2.0;

/// A preview asked for, waiting for the render, or playing.
#[derive(Debug, Clone)]
pub struct Preview {
    pub track: TrackId,
    pub note: NoteRef,
    /// The note's span in session time.
    pub start: Samples,
    pub end: Samples,
    /// What goes back when it ends: the edit selection and the stopped playhead.
    selection: Range,
    playhead: Samples,
    /// Waiting for the render since; `None` once playing.
    pending_since: Option<Instant>,
}

impl Preview {
    /// Whether it is still waiting for the render.
    pub fn pending(&self) -> bool {
        self.pending_since.is_some()
    }
}

/// Whether `clip` plays its tuning as edited: no live tuning (or Original), its render made
/// from the current inputs, or a render that failed (it plays untuned either way).
pub fn render_ready(s: &Session, clip: &Clip) -> bool {
    match soundcraft_engine::vocal::render_inputs(s, clip) {
        None => true,
        Some(inputs) => s.vocal.renders.get(&clip.id).is_some_and(|r| r.inputs == inputs) || s.vocal.failed.contains(&inputs),
    }
}

/// Note `r`'s span in session time, clamped to its clip's window.
pub fn note_span(app: &mut SoundApp, track: TrackId, r: NoteRef) -> Option<(Samples, Samples)> {
    let cvs = roll::clip_views(app, track, None);
    let cv = cvs.iter().find(|c| c.clip.id == r.clip && !c.stretched)?;
    let n = cv.a.notes.iter().find(|n| n.id == r.note)?;
    let fm = Frames::of(&cv.clip, cv.a.track.hop_samples);
    let (a, b) = (fm.clamp(fm.time(n.start)), fm.clamp(fm.time(n.end)));
    (b > a).then_some((a, b))
}

/// A parameter of the selection changed: previews its first note.
pub fn changed(app: &mut SoundApp) {
    if let Some(r) = app.vocal.selected.iter().next().copied() {
        request(app, r);
    }
}

/// Asks for a preview of note `r` (a click, or an edit): nothing with Preview off, or while
/// the transport plays or records on its own; a running preview restarts.
pub fn request(app: &mut SoundApp, r: NoteRef) {
    if !app.ui.vocal_preview || app.engine.transport.recording {
        return;
    }
    let ours = app.vocal.preview.is_some();
    if app.is_playing() && !ours {
        return;
    }
    let Some(track) = editor_track(app) else { return };
    let Some((start, end)) = note_span(app, track, r) else { return };
    // Restart: the running one ends now (and puts the playhead back) before the new one starts.
    if let Some(pv) = app.vocal.preview.take() {
        if app.is_playing() {
            app.stop_play();
        }
        finish(app, &pv);
    }
    let s = app.engine.session();
    let (selection, playhead) = (s.edit.selection, s.edit.playhead);
    app.vocal.preview = Some(Preview { track, note: r, start, end, selection, playhead, pending_since: Some(Instant::now()) });
}

/// Per frame: starts a pending preview once the render is current (or the wait is up), and
/// puts the playhead and selection back when a preview has ended.
pub fn tick(app: &mut SoundApp) {
    let Some(mut pv) = app.vocal.preview.take() else { return };
    if let Some(since) = pv.pending_since {
        if app.is_playing() || app.engine.transport.recording {
            // Someone else started the transport meanwhile: the preview is dropped.
            return;
        }
        let s = app.engine.session_arc();
        let ready = s.find_clip(pv.note.clip).is_none_or(|(_, c)| render_ready(&s, c));
        if ready || since.elapsed().as_secs_f64() >= RENDER_WAIT_SECS {
            pv.pending_since = None;
            app.preview_solo = Some(pv.track);
            app.start_play(pv.start, Some(pv.end), None);
        }
        app.vocal.preview = Some(pv);
        return;
    }
    if app.is_playing() {
        app.vocal.preview = Some(pv);
        return;
    }
    // It reached the note's end (or was stopped): back to where things were.
    finish(app, &pv);
}

/// The preview is over: the solo goes, the player hears the session again, and the playhead
/// and selection are as they were.
fn finish(app: &mut SoundApp, pv: &Preview) {
    app.preview_solo = None;
    if let Some(p) = &app.player {
        p.update_session(app.engine.session_arc());
    }
    let s = app.engine.session();
    if s.edit.selection != pv.selection || s.edit.playhead != pv.playhead {
        let s = app.engine.session_mut();
        s.edit.selection = pv.selection;
        s.edit.playhead = pv.playhead;
    }
    app.engine.transport.position = pv.playhead;
}
