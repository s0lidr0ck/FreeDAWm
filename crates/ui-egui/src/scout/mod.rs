//! Scout in the UI: arming the capture the engine asked for (`TransportRequest::ScoutStart`),
//! taking the rigs back and storing them (`scout.store`), the Scout button's state, the panel
//! (Window ▸ Scout) and its UI commands (`window.scout`, `ui.scout.package`, `ui.scout.copy`).
//!
//! Every rig that comes back is consumed here: a finished one is stored in the session it was
//! started in (the engine refuses any other, quietly here); a refused one (never armed) ends the
//! capture with a message. A rig that never comes back (the audio device stopped) is declared lost
//! after [`LOST_AFTER_SECS`] without progress, so the button never stays stuck capturing.

pub mod ask;
pub mod guides;
pub mod package;
pub mod panel;
pub mod profiles;

use crate::SoundApp;
use serde_json::{Value, json};
use soundcraft_mix::scout::ScoutRig;
use soundcraft_time::{Range, Samples};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Seconds an armed capture may go without progress before its rig is counted lost.
pub const LOST_AFTER_SECS: f64 = 4.0;
/// The status when the capture's rig never came back.
pub const LOST: &str = "Capture lost (audio device stopped)";
/// The status when the audio engine would not take the capture.
pub const REFUSED: &str = "Scout couldn't start the capture: the audio engine isn't running";

/// A capture out on the audio side.
struct Armed {
    capture: u32,
    progress: Arc<AtomicU64>,
    /// Frames as last seen, and when they last moved (UI clock, seconds).
    seen: u64,
    moved_at: f64,
}

/// The Scout window's tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Captures,
    Profiles,
}

/// The Scout panel and capture state (not saved; the captures themselves are in the session).
#[derive(Default)]
pub struct ScoutUi {
    /// Window ▸ Scout.
    pub show: bool,
    /// Captures or Profiles.
    pub tab: Tab,
    /// The Profiles tab.
    pub profiles: profiles::ProfilesUi,
    /// Captures ticked to go with the question. `None`: the newest only (the default).
    pub ticked: Option<BTreeSet<u32>>,
    /// The question being typed.
    pub question: String,
    /// The capture being renamed, and the name typed so far.
    pub renaming: Option<(u32, String)>,
    /// The last thing worth saying in the panel (lost or refused capture).
    pub message: Option<String>,
    armed: Vec<Armed>,
    /// Without a player (tests, offscreen renders): the armed rig, and rigs ended and not stored.
    sim: Option<Box<ScoutRig>>,
    sim_back: Vec<ScoutRig>,
    /// The question out with Claude, or the last one that failed.
    pub asking: Option<ask::Asking>,
    /// Scout ▸ Settings is open, and the key being typed there (memory only, never saved here).
    pub settings_open: bool,
    pub key_draft: String,
    /// The masked key as last read from the host (`None`: not read yet). See `ask::key_status`.
    pub key_status: Option<Option<String>>,
    /// The strips of the capture last started (their PRE/POST lights show while it runs).
    pub strips: Vec<soundcraft_model::TrackId>,
    /// Text Copy put up, handed to the clipboard on the next frame.
    pending_copy: Option<String>,
    /// The context of the last frame (for the package's value texts).
    ctx: Option<egui::Context>,
    now: f64,
}

impl ScoutUi {
    /// Frames captured by the running capture, if one is out.
    pub fn frames(&self) -> Option<u64> {
        self.armed.first().map(|a| a.progress.load(Ordering::Relaxed)).or_else(|| self.sim.as_ref().map(|r| r.frames()))
    }

    /// The captures to send with a question: the ticked ones that still exist, newest first; by
    /// default the newest.
    pub fn chosen(&self, s: &soundcraft_model::Session) -> Vec<u32> {
        let ids = s.scout.captures.iter().rev().map(|c| c.id);
        match &self.ticked {
            Some(t) => ids.filter(|id| t.contains(id)).collect(),
            None => ids.take(1).collect(),
        }
    }

    pub fn is_ticked(&self, s: &soundcraft_model::Session, id: u32) -> bool {
        match &self.ticked {
            Some(t) => t.contains(&id),
            None => s.scout.captures.last().is_some_and(|c| c.id == id),
        }
    }

    /// Ticks or unticks a capture (from then on the choice is the user's).
    pub fn tick(&mut self, s: &soundcraft_model::Session, id: u32, on: bool) {
        let mut t = self.ticked.take().unwrap_or_else(|| s.scout.captures.last().map(|c| c.id).into_iter().collect());
        if on {
            t.insert(id);
        } else {
            t.remove(&id);
        }
        self.ticked = Some(t);
    }
}

/// UI commands: `ui.scout.starting_prompt_copy` (→ its length), `ui.scout.starting_prompt_save
/// {path?}`, `window.scout`, `ui.scout.package {captures?, question?}` (→ the package),
/// `ui.scout.copy {captures?, question?}` (puts the prompt on the clipboard → its length) and
/// `ui.scout.send {captures?, question?}` (asks Claude; the answer streams into the panel). The
/// first three refuse an empty capture list.
pub fn run(app: &mut SoundApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    if let Some(r) = profiles::run(app, id, p) {
        return Some(r);
    }
    Some(match id {
        "window.scout" => {
            app.scout.show = p.get("value").and_then(Value::as_bool).unwrap_or(!app.scout.show);
            Ok(json!({"value": app.scout.show}))
        }
        "ui.scout.package" => package_for(app, p).map(|(pkg, _)| pkg),
        "ui.scout.copy" => package_for(app, p).map(|(pkg, question)| {
            let text = package::prompt_text(&pkg, &question);
            let len = text.len();
            app.scout.pending_copy = Some(text);
            app.ui.status = "Scout prompt and package copied".into();
            json!({"copied": len})
        }),
        "ui.scout.send" => ask::send(app, p),
        "ui.scout.starting_prompt_copy" => {
            let text = soundcraft_engine::scout_prompt::starting_prompt();
            let len = text.chars().count();
            app.scout.pending_copy = Some(text);
            app.ui.status = "Starting prompt copied".into();
            Ok(json!({"copied": len}))
        }
        "ui.scout.starting_prompt_save" => save_starting_prompt(app, p),
        _ => return None,
    })
}

/// The file name Save Starting Prompt… offers.
pub const STARTING_PROMPT_FILE: &str = "FreeDAWm Starting Prompt.md";

/// `ui.scout.starting_prompt_save {path?}`: writes the starting prompt to `path`, else to the file
/// the save dialog picks (menu use only) → {path, chars}. Cancelled: {path: null}.
fn save_starting_prompt(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let path = match p.get("path").and_then(Value::as_str) {
        Some(path) => Some(path.to_string()),
        None => app.services.pick_save.as_ref().and_then(|pick| pick("scout.starting_prompt", STARTING_PROMPT_FILE)),
    };
    let Some(path) = path else { return Ok(json!({"path": null})) };
    let text = soundcraft_engine::scout_prompt::starting_prompt();
    std::fs::write(&path, &text).map_err(|e| format!("Could not save the starting prompt to {path}: {e}"))?;
    app.ui.status = format!("Starting prompt saved to {path}");
    Ok(json!({"path": path, "chars": text.chars().count()}))
}

/// The captures named by `captures` (default: the ticked ones), those that exist.
pub(crate) fn captures_param(app: &SoundApp, p: &Value) -> Vec<u32> {
    let s = app.engine.session();
    match p.get("captures").and_then(Value::as_array) {
        Some(a) => a
            .iter()
            .filter_map(Value::as_u64)
            .filter_map(|n| u32::try_from(n).ok())
            .filter(|id| s.scout.captures.iter().any(|c| c.id == *id))
            .collect(),
        None => app.scout.chosen(s),
    }
}

/// The refusal when no capture is chosen.
pub const NO_CAPTURE_CHOSEN: &str = "Choose at least one capture";

/// The package for `captures` (default: the ticked ones) and `question` (default: the one typed).
pub(crate) fn package_for(app: &SoundApp, p: &Value) -> Result<(Value, String), String> {
    let s = app.engine.session();
    let captures = captures_param(app, p);
    if captures.is_empty() {
        return Err(NO_CAPTURE_CHOSEN.into());
    }
    let question = p.get("question").and_then(Value::as_str).map_or_else(|| app.scout.question.clone(), str::to_string);
    let ctx = app.scout.ctx.clone().unwrap_or_default();
    Ok((package::build_package(&ctx, s, &captures, &question, &s.scout.goal), question))
}

/// `TransportRequest::ScoutStart`: builds the rig here (off the audio thread) and arms it; a loop
/// capture arms, locates and plays the loop in one step.
pub(crate) fn start(app: &mut SoundApp, capture: u32, strips: &[soundcraft_model::TrackId], start: Option<Samples>, end: Option<Samples>) {
    let session = app.engine.session_arc();
    let rig = Box::new(ScoutRig::new(&session, strips, start, end).with_capture(capture));
    app.scout.strips = strips.to_vec();
    let looped = match (start, end) {
        (Some(a), Some(b)) => Some(Range::new(a, b)),
        _ => None,
    };
    app.scout.message = None;
    if let Some(p) = &app.player {
        app.scout.armed.push(Armed { capture, progress: rig.progress(), seen: 0, moved_at: app.scout.now });
        p.update_session(session);
        match looped {
            Some(r) => {
                // A refused rig leaves the transport as it was: nothing started playing.
                if p.scout_loop(rig, r) {
                    app.note_play_started(r.start);
                }
            }
            None => p.arm_scout(rig),
        }
    } else {
        // No audio engine: the capture runs on the simulated transport and measures nothing.
        if let Some(mut old) = app.scout.sim.replace(rig) {
            old.stop();
            app.scout.sim_back.push(*old);
        }
        if let Some(r) = looped {
            app.start_play(r.start, None, Some(r));
        }
    }
}

/// `TransportRequest::ScoutStop`: ends the armed capture; its rig comes back to be stored.
pub(crate) fn stop(app: &mut SoundApp) {
    if let Some(p) = &app.player {
        p.stop_scout();
    }
    if let Some(mut r) = app.scout.sim.take() {
        r.stop();
        app.scout.sim_back.push(*r);
    }
}

/// The transport stopped: on the simulated transport that ends the capture, as on the audio side.
pub(crate) fn transport_stopped(app: &mut SoundApp) {
    if app.player.is_none() {
        stop(app);
    }
}

/// The player went away or its device stopped: its rigs will never come back.
pub(crate) fn lost(app: &mut SoundApp) {
    if app.scout.armed.is_empty() {
        return;
    }
    // A rig that timed out may still be out on this player: stop it capturing.
    if let Some(p) = &app.player {
        p.stop_scout();
    }
    app.scout.armed.clear();
    end_engine_capture(app);
    app.scout.message = Some(LOST.into());
    app.ui.status = LOST.into();
}

/// Clears the engine's running capture (it would otherwise keep the button capturing).
fn end_engine_capture(app: &mut SoundApp) {
    if app.engine.scout_capture.is_some() {
        let _ = app.engine.execute("scout.capture", &json!({"action": "stop"}));
        // Nothing is armed any more: the stop has nothing to tell the player.
        app.engine.transport_requests.retain(|r| !matches!(r, soundcraft_engine::TransportRequest::ScoutStop));
    }
}

/// Stores every rig already back on the current player (before the player is replaced).
pub(crate) fn drain(app: &mut SoundApp) {
    let mut back = Vec::new();
    if let Some(p) = &app.player {
        while let Some(rig) = p.take_scout() {
            back.push(*rig);
        }
    }
    for rig in back {
        take_back(app, rig);
    }
}

/// Each frame: takes back every rig and stores it, watches for lost rigs, and hands Copy's text to
/// the clipboard.
pub(crate) fn poll(app: &mut SoundApp, ctx: &egui::Context) {
    app.scout.now = ctx.input(|i| i.time);
    app.scout.ctx = Some(ctx.clone());
    if let Some(text) = app.scout.pending_copy.take() {
        ctx.copy_text(text);
    }
    ask::poll(app);
    #[cfg(not(target_arch = "wasm32"))]
    profiles::poll(app);
    let mut back: Vec<ScoutRig> = std::mem::take(&mut app.scout.sim_back);
    if let Some(p) = &app.player {
        while let Some(rig) = p.take_scout() {
            back.push(*rig);
        }
    }
    for rig in back {
        take_back(app, rig);
    }
    watch(app, app.scout.now);
    if app.scout.frames().is_some() {
        // The button's elapsed time.
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// Declares the armed captures lost when none has moved for [`LOST_AFTER_SECS`] (UI clock `now`).
fn watch(app: &mut SoundApp, now: f64) {
    let mut stalled = false;
    for a in &mut app.scout.armed {
        let f = a.progress.load(Ordering::Relaxed);
        if f != a.seen {
            a.seen = f;
            a.moved_at = now;
        } else if now - a.moved_at > LOST_AFTER_SECS {
            stalled = true;
        }
    }
    if stalled {
        lost(app);
    }
}

/// One rig back from the audio side.
fn take_back(app: &mut SoundApp, rig: ScoutRig) {
    let id = rig.capture();
    app.scout.armed.retain(|a| a.capture != id);
    if !rig.finished() {
        // Refused: it never captured. End the capture it was for, if it is still the running one.
        if app.engine.scout_capture == Some(id) {
            end_engine_capture(app);
            app.scout.message = Some(REFUSED.into());
            app.ui.status = REFUSED.into();
        }
        return;
    }
    let played: Vec<Range> = rig.played().to_vec();
    let truncated = rig.played_truncated();
    let loop_range = match rig.bounds() {
        (Some(a), Some(b)) => Some(Range::new(a, b)),
        _ => None,
    };
    let strips = rig.finish(app.engine.session());
    let params = json!({"capture": id, "strips": strips, "played": played, "loop_range": loop_range, "played_truncated": truncated});
    match app.engine.execute("scout.store", &params) {
        Ok(_) => {}
        // A capture of a session since replaced (or of an undone one): never stored elsewhere.
        Err(e) if e.to_string().contains(soundcraft_engine::cmd::SCOUT_NOT_PENDING) => log::info!("scout.store: {e}"),
        Err(e) => {
            log::warn!("scout.store: {e}");
            app.ui.status = e.to_string();
        }
    }
}

/// The PRE/POST lights of a Mix strip being captured: PRE at the left of `row` (the strip's
/// track-type line, above the name plate), POST at its right; nothing on the other strips.
pub(crate) fn strip_lights(app: &SoundApp, ui: &egui::Ui, id: soundcraft_model::TrackId, row: egui::Rect) {
    if !capturing(app) || !app.scout.strips.contains(&id) {
        return;
    }
    let t = crate::theme::Tokens::current();
    let pre = egui::Rect::from_min_size(row.left_top(), egui::vec2(24.0, row.height()));
    let post = egui::Rect::from_min_size(egui::pos2(row.right() - 30.0, row.top()), egui::vec2(30.0, row.height()));
    for (label, pill) in [("PRE", pre), ("POST", post)] {
        ui.painter().rect_filled(pill, egui::CornerRadius::same(5), t.rec);
        ui.painter().text(pill.center(), egui::Align2::CENTER_CENTER, label, crate::theme::bold(7.5), egui::Color32::WHITE);
    }
    let resp = ui.interact(row, ui.id().with(("scout_lights", id.0)), egui::Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, "Scout PRE/POST"));
    resp.on_hover_text("Scout is capturing this strip before and after its modules");
}

/// Whether Scout is capturing (the engine's capture is running).
pub fn capturing(app: &SoundApp) -> bool {
    app.engine.scout_capture.is_some()
}

/// The loop Scout can capture whole now: loop playback on, a selection, at most 30 seconds.
pub fn capturable_loop(app: &SoundApp) -> Option<Range> {
    let s = app.engine.session();
    // The sticky loop (the last range selected), whether or not Loop Playback is on.
    let lp = s.edit.loop_range?;
    let max = (f64::from(soundcraft_dsp::scout::MAX_CAPTURE_SECONDS) * s.sample_rate.as_f64()).round() as i64;
    (!lp.is_empty() && lp.len() <= max).then_some(lp)
}

/// `m:ss` of a frame count at `sr`.
pub fn clock(frames: u64, sr: f64) -> String {
    let secs = if sr > 0.0 { (frames as f64 / sr).floor() as u64 } else { 0 };
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;
    use soundcraft_model::{ChannelFormat, Session, TrackKind};

    fn app() -> (SoundApp, soundcraft_model::TrackId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
        s.edit.selected_tracks = vec![t];
        (SoundApp::new(soundcraft_engine::Engine::new(s), None, Services::default()), t)
    }

    fn start_capture(app: &mut SoundApp) -> u32 {
        let id = app.engine.execute("scout.capture", &json!({"action": "start"})).unwrap()["capture"].as_u64().unwrap() as u32;
        app.engine.transport_requests.clear();
        id
    }

    #[test]
    fn a_rig_that_stops_moving_is_lost_and_the_button_comes_back() {
        let (mut app, _) = app();
        let id = start_capture(&mut app);
        app.scout.armed.push(Armed { capture: id, progress: Arc::new(AtomicU64::new(0)), seen: 0, moved_at: 10.0 });
        watch(&mut app, 12.0);
        assert!(capturing(&app), "still within the grace period");
        // Progress keeps it alive.
        app.scout.armed[0].progress.store(512, Ordering::Relaxed);
        watch(&mut app, 13.0);
        watch(&mut app, 13.0 + LOST_AFTER_SECS - 0.5);
        assert!(capturing(&app));
        watch(&mut app, 13.0 + LOST_AFTER_SECS + 0.5);
        assert!(!capturing(&app), "the button is not stuck capturing");
        assert_eq!(app.scout.message.as_deref(), Some(LOST));
        assert_eq!(app.ui.status, LOST);
        assert!(app.scout.armed.is_empty());
        assert!(app.engine.transport_requests.is_empty(), "nothing to stop on the player");
    }

    #[test]
    fn a_dead_device_loses_the_capture_at_once() {
        let (mut app, _) = app();
        let id = start_capture(&mut app);
        app.scout.armed.push(Armed { capture: id, progress: Arc::new(AtomicU64::new(0)), seen: 0, moved_at: 0.0 });
        lost(&mut app);
        assert!(!capturing(&app));
        assert_eq!(app.scout.message.as_deref(), Some(LOST));
        // Nothing out: nothing to say.
        app.scout.message = None;
        lost(&mut app);
        assert_eq!(app.scout.message, None);
    }

    #[test]
    fn a_refused_rig_ends_its_capture_with_a_message() {
        let (mut app, t) = app();
        let id = start_capture(&mut app);
        let rig = ScoutRig::new(app.engine.session(), &[t], None, None).with_capture(id);
        assert!(!rig.finished());
        take_back(&mut app, rig);
        assert!(!capturing(&app));
        assert_eq!(app.scout.message.as_deref(), Some(REFUSED));
        assert!(app.engine.session().scout.captures.is_empty());
    }

    #[test]
    fn a_rig_of_a_replaced_session_is_dropped_quietly() {
        let (mut app, t) = app();
        let id = start_capture(&mut app);
        start(&mut app, id, &[t], None, None);
        stop(&mut app);
        let rig = app.scout.sim_back.pop().expect("the stopped rig");
        assert!(rig.finished());
        let mut s = Session::default();
        let v = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
        s.edit.selected_tracks = vec![v];
        app.engine.replace_session(s);
        // The new session starts a capture of its own before the old rig is back.
        let fresh = start_capture(&mut app);
        assert_ne!(fresh, id, "ids are never reused across sessions");
        app.ui.status.clear();
        take_back(&mut app, rig);
        assert!(app.engine.session().scout.captures.is_empty(), "never stored in another session");
        assert!(app.ui.status.is_empty(), "quietly");
        assert!(capturing(&app), "the new session's capture is untouched");
    }

    #[test]
    fn every_rig_that_comes_back_is_stored() {
        let (mut app, t) = app();
        let a = start_capture(&mut app);
        start(&mut app, a, &[t], None, None);
        stop(&mut app);
        assert!(app.engine.execute("scout.capture", &json!({"action": "stop"})).is_ok());
        let b = start_capture(&mut app);
        start(&mut app, b, &[t], None, None);
        stop(&mut app);
        // Both back in one frame.
        let back = std::mem::take(&mut app.scout.sim_back);
        assert_eq!(back.len(), 2);
        for rig in back {
            take_back(&mut app, rig);
        }
        let ids: Vec<u32> = app.engine.session().scout.captures.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec![a, b]);
        assert!(!capturing(&app));
    }
}
