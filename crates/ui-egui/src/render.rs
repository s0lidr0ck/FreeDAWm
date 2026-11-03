//! Track › Freeze, Freeze up to here, Commit and Bounce (like File › Bounce Mix, see `bounce`)
//! render through the plugin instances that are playing, Pro Tools offline-bounce style: the
//! transport stops, the player lends its engine (every hosted instance in it) to the render,
//! the render runs a slice per frame behind a progress window, and the engine goes back with
//! every instance's state as it was. While a render runs the transport cannot start (the status
//! line reads "Rendering…") and no other render starts.

use crate::SoundApp;
use crate::theme::{Tokens, bold};
use serde_json::{Value, json};
use soundcraft_engine::render_job::RenderJob;

/// What the status line says while a render runs, and why the transport is refused.
pub const RENDERING: &str = "Rendering…";

/// How long a render waits for the player's engine before rendering through instances of its own
/// (the audio thread is not answering: a stalled device).
const LEND_PATIENCE: std::time::Duration = std::time::Duration::from_secs(3);

/// Commands a running render refuses: anything that starts the transport, another render, and
/// replacing the session under it.
pub fn blocked(id: &str) -> bool {
    matches!(
        id,
        "transport.play"
            | "transport.play_selection"
            | "transport.toggle"
            | "transport.record"
            | "transport.half_speed"
            | "track.freeze"
            | "track.commit"
            | "track.bounce"
            | "file.bounce_mix"
            | "session.new"
            | "session.open"
            | "session.close"
            | "session.revert"
    )
}

/// Starts the render `id` (`track.freeze`, `track.commit`, `track.bounce`) runs for `params`:
/// the transport stops, the player lends its instances, and the command itself runs once the
/// render is done ([`step`]). `None` when the command renders nothing (an unfreeze) or there is
/// no player: run it as it is. The result is what the caller returns for the command.
pub fn start(app: &mut SoundApp, id: &str, params: &Value) -> Option<Result<Value, String>> {
    app.player.as_ref()?;
    if crate::open_progress::refuses(app) {
        return Some(Err(app.ui.status.clone()));
    }
    let job = match RenderJob::new(&app.engine, id, params) {
        Ok(Some(job)) => job,
        Ok(None) => return None,
        Err(e) => {
            app.ui.status = e.to_string();
            return Some(Err(e.to_string()));
        }
    };
    app.stop_play();
    // Third-party plugins keep state the session can't see: store it first (a save during the
    // render, and a render through instances of its own, read it).
    app.capture_plugin_states();
    app.track_render = Some(job);
    lend(app);
    app.ui.status = RENDERING.into();
    Some(Ok(json!({"rendering": true})))
}

/// Asks the player for its live instances for the render just started ([`start`], `bounce::start`).
pub(crate) fn lend(app: &mut SoundApp) {
    let Some(p) = &app.player else { return };
    match p.lend_instances(app.engine.session_arc()) {
        Ok(l) => app.lending = Some(l),
        Err(e) => log::warn!("render through the live instances refused: {e}; rendering through copies"),
    }
}

/// True once the render can go on: the player's instances arrived (and went to the job), or the
/// render does without them. False while they are still on their way (a frame or two).
pub(crate) fn instances_ready(app: &mut SoundApp, ctx: &egui::Context) -> bool {
    instances_ready_within(app, ctx, LEND_PATIENCE)
}

/// [`instances_ready`] with the patience given (tests shorten it).
fn instances_ready_within(app: &mut SoundApp, ctx: &egui::Context, patience: std::time::Duration) -> bool {
    let Some(lending) = &mut app.lending else { return true };
    match lending.poll() {
        Some(live) => {
            app.lending = None;
            if let Some(job) = app.track_render.take() {
                app.track_render = Some(job.through(live));
            } else if let Some(job) = app.bounce.take() {
                app.bounce = Some(job.through(live));
            }
            true
        }
        None if lending.waited() > patience => {
            log::warn!("the audio thread did not lend its instances in time; rendering through copies");
            app.lending = None;
            true
        }
        None => {
            ctx.request_repaint();
            false
        }
    }
}

/// Renders the running track render for a moment; once done, runs its command. Called from
/// `logic` every frame.
pub fn step(app: &mut SoundApp, ctx: &egui::Context) {
    if app.track_render.is_none() || !instances_ready(app, ctx) {
        return;
    }
    let Some(job) = &mut app.track_render else { return };
    if !job.step(crate::bounce::slice()) {
        ctx.request_repaint();
        return;
    }
    let Some(job) = app.track_render.take() else { return };
    let (id, params) = job.command();
    let (id, params) = (id, params.clone());
    job.finish(&mut app.engine);
    // The command finds its renders ready (`Engine::take_prepared`); the status line says what
    // it did.
    match app.run_command(id, params) {
        Ok(v) if id != "track.freeze" => app.ui.status = commit_status(&app.engine, &v),
        Ok(_) => {}
        Err(e) => app.ui.status = e,
    }
}

/// The status line after `track.commit` / `track.bounce` returned `v`.
fn commit_status(e: &soundcraft_engine::Engine, v: &Value) -> String {
    let names: Vec<String> = v
        .get("tracks")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_u64).filter_map(|t| e.session().track(soundcraft_model::TrackId(t)).map(|t| t.name.clone())).collect())
        .unwrap_or_default();
    if names.is_empty() { "Nothing to commit".into() } else { format!("Committed to {}", names.join(", ")) }
}

/// Shows how far the running track render got, with Cancel (or Esc). Cancelling hands the
/// instances back untouched; nothing is frozen or committed.
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(job) = &app.track_render else { return };
    let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    egui::Modal::new(egui::Id::new("render_progress")).show(ctx, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        ui.set_width(320.0);
        ui.label(egui::RichText::new(job.label()).font(bold(13.0)));
        ui.add(egui::ProgressBar::new(job.progress()));
        ui.horizontal(|ui| {
            cancel |= ui.button("Cancel").clicked();
            let through = if job.is_live() { "through the live plugins" } else { "" };
            ui.label(egui::RichText::new(format!("{:.0}% {through}", job.progress() * 100.0)).color(Tokens::current().text_dim));
        });
    });
    if cancel {
        app.track_render = None;
        app.lending = None;
        app.ui.status = "Render cancelled".into();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A session with an instrument track (the built-in synth playing one note).
    fn app() -> SoundApp {
        use soundcraft_midi::{Note, Sequence};
        use soundcraft_model::{ChannelFormat, Clip, Insert, Session, TrackKind};
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let mut seq = Sequence::default();
        seq.notes.push(Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 });
        let cid = s.new_clip_id();
        let tr = s.track_mut(t).unwrap();
        tr.playlist_mut().unwrap().clips.push(Clip::midi(cid, "n", 0, 24_000, seq));
        tr.instrument = Some(Insert::new("subtractive_synth"));
        let engine = soundcraft_engine::Engine::new(s);
        let player = soundcraft_playback::Player::new(engine.session_arc());
        SoundApp::new(engine, Some(player), crate::Services::default())
    }

    /// Steps the render until it is done (the player's engine arrives from the audio thread).
    fn finish(app: &mut SoundApp, ctx: &egui::Context) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.track_render.is_some() {
            assert!(std::time::Instant::now() < deadline, "the render never finished");
            step(app, ctx);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn a_freeze_renders_as_a_job_and_the_transport_cannot_start_meanwhile() {
        let mut app = app();
        let ctx = egui::Context::default();
        app.run("transport.play", json!({})).unwrap();
        app.handle_transport(1.0 / 60.0);
        assert!(app.is_playing());
        // Freeze while playing: the transport stops, the render starts.
        let v = app.run("track.freeze", json!({"tracks": ["Keys"]})).unwrap();
        assert_eq!(v, json!({"rendering": true}));
        assert!(!app.is_playing());
        assert!(app.track_render.is_some());
        assert_eq!(app.ui.status, RENDERING);
        assert_eq!(app.run("transport.play", json!({})).unwrap_err(), RENDERING);
        assert_eq!(app.run("track.commit", json!({"tracks": ["Keys"]})).unwrap_err(), RENDERING);
        app.handle_transport(1.0 / 60.0);
        assert!(!app.is_playing(), "nothing started");
        finish(&mut app, &ctx);
        let s = app.engine.session();
        assert!(s.tracks[0].frozen_audio().is_some(), "frozen once the render is done");
        assert!(app.ui.status.starts_with("Froze Keys"), "{}", app.ui.status);
        assert!(app.lending.is_none());
        assert!(!app.player.as_ref().unwrap().is_lent(), "the engine is back");
        app.run("transport.play", json!({})).unwrap();
        app.handle_transport(1.0 / 60.0);
        assert!(app.is_playing(), "and the transport starts again");
    }

    #[test]
    fn the_audio_device_cannot_change_while_a_render_holds_the_players_instances() {
        let mut app = app();
        let ctx = egui::Context::default();
        app.run("track.freeze", json!({"tracks": ["Keys"]})).unwrap();
        assert!(app.rendering());
        let name = app.player.as_ref().unwrap().device_name.clone();
        assert_eq!(app.apply_audio_config(soundcraft_playback::AudioConfig::default()).unwrap_err(), crate::RENDER_HOLDS_DEVICE);
        app.reconnect_audio();
        assert_eq!(app.ui.status, crate::RENDER_HOLDS_DEVICE);
        assert_eq!(app.player.as_ref().unwrap().device_name, name, "the player is the same");
        finish(&mut app, &ctx);
        assert!(!app.rendering());
        app.reconnect_audio();
        assert_ne!(app.ui.status, crate::RENDER_HOLDS_DEVICE, "allowed again once the render is done");
    }

    #[test]
    fn a_render_the_audio_thread_does_not_lend_to_in_time_goes_on_through_copies() {
        // A stalled device never answers the lend: after the patience runs out the render goes
        // on with instances of its own (the built-in synth here), and the engine, when it does
        // arrive, goes straight back (the player keeps polling for it).
        let mut app = app();
        let ctx = egui::Context::default();
        // A player whose audio thread is stalled: it never drains its commands.
        let (player, mut st) = soundcraft_playback::Player::stalled(app.engine.session_arc(), 48_000, 256, 2);
        app.player = Some(player);
        app.run("track.freeze", json!({"tracks": ["Keys"]})).unwrap();
        assert!(app.lending.is_some());
        assert!(!instances_ready_within(&mut app, &ctx, std::time::Duration::from_secs(60)), "still waiting");
        assert!(app.track_render.is_some());
        assert!(instances_ready_within(&mut app, &ctx, std::time::Duration::ZERO), "patience over: on with copies");
        assert!(app.lending.is_none());
        assert!(!app.track_render.as_ref().unwrap().is_live());
        assert!(app.player.as_ref().unwrap().is_lent(), "the lend is still out there");
        finish(&mut app, &ctx);
        assert!(app.engine.session().tracks[0].frozen_audio().is_some(), "frozen through copies");
        // The device recovers: the lend is answered late and the engine goes straight back.
        let mut out = vec![0.0f32; 2 * 128];
        st.fill(&mut out, 2);
        let _ = app.player.as_ref().unwrap().idle();
        assert!(!app.player.as_ref().unwrap().is_lent(), "the late engine was sent back");
        st.fill(&mut out, 2);
        app.run("transport.play", json!({})).unwrap();
        app.handle_transport(1.0 / 60.0);
        st.fill(&mut out, 2);
        assert!(app.is_playing(), "and the transport starts");
    }

    #[test]
    fn an_unfreeze_and_a_cancelled_render_change_nothing_but_the_status() {
        let mut app = app();
        let ctx = egui::Context::default();
        app.run("track.freeze", json!({"tracks": ["Keys"]})).unwrap();
        assert!(app.track_render.is_some());
        app.track_render = None;
        app.lending = None;
        app.ui.status = "Render cancelled".into();
        finish(&mut app, &ctx);
        assert!(app.engine.session().tracks[0].frozen_audio().is_none(), "nothing frozen");
        let _ = app.player.as_ref().unwrap().idle();
        // A second freeze right after: the engine came back.
        app.run("track.freeze", json!({"tracks": ["Keys"]})).unwrap();
        finish(&mut app, &ctx);
        assert!(app.engine.session().tracks[0].frozen_audio().is_some());
        // Unfreeze renders nothing: the command runs at once.
        let v = app.run("track.freeze", json!({"tracks": ["Keys"]})).unwrap();
        assert_eq!(v["unfrozen"], json!([app.engine.session().tracks[0].id.0]));
        assert!(app.track_render.is_none());
    }
}
