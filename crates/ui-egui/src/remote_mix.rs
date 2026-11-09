//! Remote mixing, the remote PC's side (spec `docs/superpowers/specs/2026-11-09-remote-mixing-design.md`).
//!
//! While this PC mixes another (Setup › Network › Mix another PC), its engine holds a copy of
//! the host's session, kept up to date from the host ([`MixerClient`]); this PC's own session is
//! put aside and comes back when mixing ends. Commands go where [`route`] says through the
//! engine's relay: a mix or automation change runs here at once and at the host, the transport
//! only at the host, editing is refused. The player is handed an empty session at the host's
//! rate, so nothing renders here: what is heard is the host's mix stream. The meters, the
//! playhead and the transport come from the host, delayed by the stream's buffer so they move
//! with what is heard. A control held still here (a fader in Touch) keeps touching it at the
//! host, renewed every 200 ms, so a dropped link lets go within half a second.

use crate::SoundApp;
use serde_json::{Value, json};
use soundcraft_engine::{Engine, Relay};
use soundcraft_model::Session;
use soundcraft_net::mixer::{MixerClient, MixerTarget, REFUSED, Route, route};
use std::time::{Duration, Instant};

/// How often a control held here touches it again at the host (a touch lasts half a second).
const HOLD_EVERY: Duration = Duration::from_millis(200);

pub struct RemoteMix {
    pub client: MixerClient,
    /// This PC's own engine (its session, history and settings), put back when mixing ends.
    local: Option<Engine>,
    /// Hear the host's mix.
    pub hear: bool,
    held_at: Option<Instant>,
    /// The host's session arrived at least once.
    pub ready: bool,
}

/// Starts mixing the host `target`: this PC's session is put aside.
pub fn start(app: &mut SoundApp, target: MixerTarget, waker: Option<soundcraft_net::hub::Waker>) -> Result<(), String> {
    if app.engine.transport.recording {
        return Err("Stop recording before mixing another PC".into());
    }
    stop(app);
    if app.is_playing() {
        app.stop_play();
    }
    let client = MixerClient::start(target);
    client.set_waker(waker);
    let mut mirror = Engine::default();
    let relay_client = client.clone();
    mirror.relay = Some(Box::new(move |id: &str, p: &Value, merge: Option<&str>| match route(id) {
        Route::Local => Relay::Local,
        Route::Refuse => Relay::Refuse(REFUSED.into()),
        Route::Both | Route::Remote if !relay_client.send_command(id, p, merge) => Relay::Refuse("not connected to the PC being mixed".into()),
        Route::Both => Relay::Both,
        Route::Remote => Relay::Remote,
    }));
    let local = std::mem::replace(&mut app.engine, mirror);
    app.remote_mix = Some(RemoteMix { client, local: Some(local), hear: true, held_at: None, ready: false });
    forget_views(app);
    app.ui.window = crate::MainWindow::Mix;
    Ok(())
}

/// Stops mixing remotely: this PC's own session comes back.
pub fn stop(app: &mut SoundApp) {
    let Some(mut rm) = app.remote_mix.take() else { return };
    rm.client.stop();
    rm.client.set_stream(false, None, 0);
    if let Some(local) = rm.local.take() {
        app.engine = local;
    }
    app.engine.transport.playing = false;
    forget_views(app);
}

/// What the UI keeps about the session on screen (another session is put in place).
fn forget_views(app: &mut SoundApp) {
    // Hand the player the session in place now, whatever revision it has.
    app.last_rev = app.engine.revision.wrapping_sub(1);
    app.solo_cache = None;
    app.meters.clear();
    app.main_meter = crate::MeterDisplay::default();
    app.gesture = None;
    app.ui.drawers.clear();
}

/// Whether this PC is mixing another now.
pub fn active(app: &SoundApp) -> bool {
    app.remote_mix.is_some()
}

/// The session the player is handed while mixing remotely: nothing to render, at the host's rate.
pub fn player_session(app: &SoundApp) -> std::sync::Arc<Session> {
    std::sync::Arc::new(Session::new("Remote mix", app.engine.session().sample_rate))
}

/// Once a frame: the host's session when a newer one came, its mix stream, errors.
pub fn step(app: &mut SoundApp) {
    let Some(rm) = &mut app.remote_mix else { return };
    let client = rm.client.clone();
    match client.take_session() {
        Some(Ok(s)) => {
            rm.ready = true;
            app.engine.mirror_session(s);
        }
        Some(Err(e)) => {
            log::warn!("remote mixing: {e}");
            app.ui.status = e;
        }
        None => {}
    }
    if let Some(e) = client.take_errors().pop() {
        app.ui.status = format!("The PC being mixed refused: {e}");
    }
    let hear = app.remote_mix.as_ref().is_some_and(|r| r.hear);
    let rate = app.engine.session().sample_rate.hz();
    client.set_stream(hear, app.player.as_ref().map(soundcraft_playback::Player::stream_in), rate);
}

/// The host's meters and transport in place of the player's (in the frame's transport handling).
pub fn read_back(app: &mut SoundApp, dt: f32, b: crate::preferences::Ballistics) {
    let Some(rm) = &app.remote_mix else { return };
    let delay = rm.client.stream_delay().unwrap_or_default();
    let frame = rm.client.meters(delay);
    let connected = rm.client.connected();
    match frame.filter(|_| connected) {
        Some(f) => {
            app.engine.transport.playing = f.playing;
            app.engine.transport.recording = f.recording;
            app.engine.transport.position = f.position;
            let mut seen = Vec::with_capacity(f.tracks.len());
            for m in &f.tracks {
                let id = soundcraft_model::TrackId(m.id);
                seen.push(id);
                crate::feed_meter(app.meters.entry(id).or_default(), [m.l, m.r], m.gr, dt, b);
            }
            for (id, d) in app.meters.iter_mut().filter(|(id, _)| !seen.contains(id)) {
                let _ = id;
                crate::feed_meter(d, [0.0, 0.0], 0.0, dt, b);
            }
            crate::feed_meter(&mut app.main_meter, [f.main.l, f.main.r], 0.0, dt, b);
        }
        None => {
            app.engine.transport.playing = false;
            app.engine.transport.recording = false;
            for d in app.meters.values_mut() {
                crate::feed_meter(d, [0.0, 0.0], 0.0, dt, b);
            }
            crate::feed_meter(&mut app.main_meter, [0.0, 0.0], 0.0, dt, b);
        }
    }
}

/// The controls held here this frame keep touching them at the host (in place of this PC's own
/// automation pass: the host writes).
pub fn hold(app: &mut SoundApp, held: &[(soundcraft_model::TrackId, soundcraft_model::AutoParam)]) {
    let Some(rm) = &mut app.remote_mix else { return };
    if held.is_empty() || rm.held_at.is_some_and(|t| t.elapsed() < HOLD_EVERY) {
        return;
    }
    rm.held_at = Some(Instant::now());
    for (t, p) in held {
        rm.client.send_command("automation.touch", &json!({"tracks": [t.0], "param": p.id()}), None);
    }
}

/// The line the Mix window shows while mixing another PC.
pub fn banner(app: &SoundApp) -> Option<String> {
    let rm = app.remote_mix.as_ref()?;
    let t = rm.client.target();
    let name = if t.host_name.is_empty() { t.addr.clone() } else { t.host_name.clone() };
    Some(if !rm.client.connected() {
        format!("Mixing {name} remotely: {}…", rm.client.status())
    } else if !rm.ready {
        format!("Mixing {name} remotely: waiting for its session…")
    } else {
        format!("Mixing {name} remotely: changes go to {name}")
    })
}
