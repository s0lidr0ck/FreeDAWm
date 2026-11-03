//! A slider's drag as one undo step, shared by MIDI Cleanup's and the Vocal Editor's panels: the
//! command runs merged at most every [`DRAG_EVERY`] while dragging and always on release.
//!
//! The release runs after the app's per-frame `end_merge` (the pointer is up by then), so its
//! change joins the drag's step with `execute_joined`, or makes its own when the drag sent nothing.

use crate::SoundApp;
use egui::{Rect, Response, Ui};
use serde_json::Value;

/// How often a drag re-sends its command (seconds); the slider itself follows every frame.
pub const DRAG_EVERY: f64 = 0.03;

/// Shows a refused command's reason in the status bar (MIDI Cleanup's and the Tuning panel's
/// controls, and their slider sends).
pub fn report(app: &mut SoundApp, r: Result<Value, String>) {
    if let Err(e) = r {
        app.ui.status = e;
    }
}

/// How this frame's command joins the gesture's undo step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// Mid-drag: merged into the gesture's step under its merge key.
    Merged,
    /// The release after the drag sent something: joined onto that step.
    Joined,
    /// A click, or a release when the drag sent nothing: its own step.
    Single,
}

impl How {
    /// Runs command `id` with `params` this way; `merge` names the gesture's merge group.
    pub fn run(self, app: &mut SoundApp, id: &str, params: &Value, merge: &str) -> Result<Value, String> {
        let r = match self {
            How::Merged => app.engine.execute_merged(id, params, merge),
            How::Joined => app.engine.execute_joined(id, params),
            How::Single => app.engine.execute(id, params),
        };
        r.map_err(|e| e.to_string())
    }
}

/// One slider's gesture. `value_at` maps the pointer's position across `r` (0–1) to a value;
/// `wanted(v, done, ran)` says whether a due send should run (`done`: the gesture ends this frame,
/// `ran`: it already ran the command); `send` runs it. Returns the value to draw while the gesture
/// lasts, `None` when there is none. `id` keys the gesture's state.
#[allow(clippy::too_many_arguments)]
pub fn drive<T: Copy + Send + Sync + 'static>(
    app: &mut SoundApp,
    ui: &Ui,
    resp: &Response,
    r: Rect,
    id: egui::Id,
    value_at: impl Fn(f32) -> T,
    wanted: impl Fn(T, bool, bool) -> bool,
    mut send: impl FnMut(&mut SoundApp, T, How) -> Result<Value, String>,
) -> Option<T> {
    // (value shown, time of the last send, whether this gesture has sent).
    type Pending<T> = (T, f64, bool);
    let pending: Option<Pending<T>> = ui.ctx().data(|d| d.get_temp(id));
    let done = resp.drag_stopped() || resp.clicked();
    if !(done || resp.dragged() || resp.drag_started()) {
        if pending.is_some() {
            ui.ctx().data_mut(|d| d.remove::<Pending<T>>(id));
        }
        return None;
    }
    if resp.drag_started() {
        app.engine.end_merge();
    }
    let at = resp.interact_pointer_pos().map(|p| value_at(((p.x - r.left()) / r.width().max(1.0)).clamp(0.0, 1.0)));
    let v = at.or(pending.map(|p| p.0))?;
    let now = ui.input(|i| i.time);
    let mut ran = pending.is_some_and(|p| p.2);
    let due = done || pending.is_none_or(|p| now - p.1 >= DRAG_EVERY);
    if due && wanted(v, done, ran) {
        let how = if !done {
            How::Merged
        } else if ran {
            How::Joined
        } else {
            How::Single
        };
        let r = send(app, v, how);
        ran |= r.is_ok();
        report(app, r);
    }
    if done {
        ui.ctx().data_mut(|d| d.remove::<Pending<T>>(id));
        app.engine.end_merge();
    } else {
        let sent = if due { now } else { pending.map_or(now, |p| p.1) };
        ui.ctx().data_mut(|d| d.insert_temp::<Pending<T>>(id, (v, sent, ran)));
        ui.ctx().request_repaint();
    }
    Some(v)
}
