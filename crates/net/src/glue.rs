//! What the app does with the network once a frame, on its UI thread, written against
//! [`RigHost`] (the app's engine and transport) so tests run it on plain engines.
//!
//! - [`BoothGlue`]: answers the service remote's buttons, lands remote takes, tells followers
//!   about the transport (a new take number at each Record, where it sits on the timeline, where
//!   it stopped) and publishes the status feed (~15 Hz).
//! - [`FollowerGlue`]: a remote recorder following the booth: records, plays and stops with it
//!   at the booth's position (seconds on the booth timeline), sends the live sketch while
//!   recording, and queues each finished take with its sync points; ends a take on its own when
//!   the booth has been away for the contact timeout (2 h).

use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_engine::midi_record::{MidiTake, RecordedEvent, armed_midi_tracks, played_at};
use soundcraft_model::{Route, TrackId, TrackKind};
use soundcraft_time::{Range, Samples};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::follower::Follower;
use crate::hub::{Hub, HubRequest, PreparedAudio, ProjectInfo};
use crate::proto::{BoothTransport, FollowerReport, MarkerStatus, RemoteAction, RigStatus, SketchNote, TrackStatus, TransportState};
use crate::take::{AudioTake, BoothAnchor, TakeEnvelope, TakePayload, WireEvent, land, land_audio, remote_port_name};

/// The app as the network glue sees it.
pub trait RigHost {
    fn engine(&self) -> &Engine;
    fn engine_mut(&mut self) -> &mut Engine;
    /// Runs a command as a click would (transport requests take effect in the host's frame).
    fn run(&mut self, id: &str, params: Value) -> Result<Value, String>;
    fn view(&self) -> HostView;
    /// A track's meter, 0..1.
    fn level(&self, _track: TrackId) -> f32 {
        0.0
    }
    /// Every track's meter and the main's (linear peaks), for remote mixing.
    fn mix_meters(&self) -> crate::mixer::MeterFrame {
        crate::mixer::MeterFrame::default()
    }
    fn disk_free(&self) -> Option<u64> {
        None
    }
    /// The local MIDI take that just finished (once), as recorded: the events as heard and where
    /// they go.
    fn take_finished(&mut self) -> Option<FinishedTake> {
        None
    }
    /// The live MIDI heard since recording started, from index `from` on (for the sketch).
    fn live_since(&self, _from: usize) -> Vec<RecordedEvent> {
        Vec::new()
    }
    /// The MIDI input ports this PC records from.
    fn midi_ports(&self) -> Vec<String> {
        Vec::new()
    }
    /// A line for the status bar.
    fn notify(&mut self, _text: String) {}
    /// A remote take is about to be queued: keep the local session (the take's backup) on disk.
    /// Returns the session's project folder, where the take waiting to be sent is kept too.
    fn keep_take(&mut self, _booth_session: &str) -> Option<std::path::PathBuf> {
        None
    }
    /// The local audio takes that just finished (once), with where they sit.
    fn audio_finished(&mut self) -> Option<FinishedAudio> {
        None
    }
    /// The inputs of the armed audio tracks (as `Remote: <PC> › <input>` names them on the booth).
    fn audio_inputs(&self) -> Vec<String> {
        armed_audio_inputs(self.engine())
    }
    /// What the audio takes recording now hold since block `from` of each (`(track, block)`): the
    /// live level for the booth.
    fn live_audio(&self, _from: &[(TrackId, usize)]) -> Vec<LiveAudio> {
        Vec::new()
    }
    /// The session's project folder (`None` while never saved).
    fn project_folder(&self) -> Option<std::path::PathBuf> {
        self.engine().path.as_deref().and_then(|p| std::path::Path::new(p).parent()).map(std::path::Path::to_path_buf)
    }
}

/// The inputs of the armed audio tracks: a hardware input by name, else `In 1` / `In 1-2`.
pub fn armed_audio_inputs(e: &Engine) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for t in e.session().tracks.iter().filter(|t| t.mixer.record_arm && t.kind == TrackKind::Audio && !t.inactive) {
        let name = audio_input_name(t);
        if !v.contains(&name) {
            v.push(name);
        }
    }
    v
}

/// A track's audio input as a remote recorder names it to the booth.
pub fn audio_input_name(t: &soundcraft_model::Track) -> String {
    match &t.mixer.input {
        Route::Hardware(n) => n.clone(),
        _ if t.channels() >= 2 => "In 1-2".into(),
        _ => "In 1".into(),
    }
}

/// A local audio take as recorded: each armed track's file, placed as the recorder planned it
/// (frame `k` at timeline `start + k - latency`).
#[derive(Debug, Clone, PartialEq)]
pub struct FinishedAudio {
    pub start: Samples,
    pub latency: Samples,
    pub rate: u32,
    pub takes: Vec<(TrackId, String, std::path::PathBuf)>,
}

/// One audio take recording now, summed up for the live level: peaks (0..1, all channels) of
/// blocks of `block` frames from block `from` on, the take's frame 0 at timeline
/// `start - latency`.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveAudio {
    pub track: TrackId,
    pub input: String,
    pub from: usize,
    pub peaks: Vec<f32>,
    pub block: usize,
    pub start: Samples,
    pub latency: Samples,
}

/// The host's transport.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HostView {
    pub playing: bool,
    pub recording: bool,
    /// The playhead (session samples).
    pub position: Samples,
    /// Where the transport started (the take clock's zero), and where recording started.
    pub play_from: Samples,
    pub record_start: Samples,
    pub looped: Option<Range>,
    pub punch: Option<Range>,
    /// A count-off runs before the take starts (`recording` and `playing` are already set, the
    /// transport holds at the record point): the take has not started yet.
    pub counting_off: bool,
}

/// A local MIDI take as the host recorded it.
#[derive(Debug, Clone, PartialEq)]
pub struct FinishedTake {
    pub take: MidiTake,
    pub events: Vec<RecordedEvent>,
}

fn rate(e: &Engine) -> f64 {
    f64::from(e.session().sample_rate.hz().max(1))
}

fn secs(e: &Engine, s: Samples) -> f64 {
    s as f64 / rate(e)
}

fn samples(e: &Engine, secs: f64) -> Samples {
    if secs.is_finite() { (secs * rate(e)).round().clamp(-9.0e15, 9.0e15) as Samples } else { 0 }
}

fn state_of(v: &HostView) -> TransportState {
    // During a count-off nothing rolls yet: the booth's take (and its anchor, and the followers'
    // takes) start when the transport rolls from the record point, not at the Record press.
    if v.counting_off {
        TransportState::Stopped
    } else if v.recording {
        TransportState::Recording
    } else if v.playing {
        TransportState::Playing
    } else {
        TransportState::Stopped
    }
}

/// `h:mm:ss` (or `m:ss`).
pub fn clock_text(secs: f64) -> String {
    let t = secs.max(0.0) as u64;
    let (h, m, s) = (t / 3600, (t / 60) % 60, t % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// A new booth take number: milliseconds since 1970, so a restarted booth never reuses one.
fn new_take_id(last: u64) -> u64 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    now.max(last.saturating_add(1))
}

/// Tracks the service remote can arm (audio, MIDI and instrument tracks, not frozen or inactive).
fn armable(e: &Engine) -> Vec<TrackId> {
    e.session()
        .tracks
        .iter()
        .filter(|t| matches!(t.kind, TrackKind::Audio | TrackKind::Midi | TrackKind::Instrument) && !t.inactive && t.frozen_audio().is_none())
        .map(|t| t.id)
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The booth

#[derive(Debug, Default)]
pub struct BoothGlue {
    take: u64,
    anchor: Option<BoothAnchor>,
    was: TransportState,
    last_sent: Option<(TransportState, u64, Option<BoothAnchor>)>,
    last_pos: Samples,
    record_began: Option<Instant>,
    last_publish: Option<Instant>,
    /// Remote mixing: the revision last published for the mixers and when, and the meters.
    mix_rev: Option<u64>,
    mix_at: Option<Instant>,
    meters_at: Option<Instant>,
}

/// The most the session goes to the mixers (a fader drag changes it every frame).
const MIX_EVERY: Duration = Duration::from_millis(50);
/// The meters' rate for the mixers.
const METERS_EVERY: Duration = Duration::from_millis(33);

impl BoothGlue {
    /// One frame. Returns the paired PCs' control-protocol requests, for the app's control
    /// handler (they answer on their own `reply`).
    pub fn step(&mut self, host: &mut dyn RigHost, hub: &Hub) -> Vec<HubRequest> {
        let mut control = Vec::new();
        for r in hub.drain() {
            match r {
                HubRequest::Control { .. } => control.push(r),
                HubRequest::Action { action, device, reply } => {
                    let got = self.action(host, action);
                    if let Err(e) = &got {
                        log::info!("service remote ({device}): {e}");
                    }
                    let _ = reply.send(got);
                }
                HubRequest::Take { take, prepared, reply } => {
                    let _ = reply.send(self.land(host, hub, &take, prepared.as_ref()));
                }
            }
        }
        self.transport(host, hub);
        self.publish(host, hub, false);
        self.publish_mix(host, hub);
        let s = host.engine().session();
        hub.set_project(ProjectInfo { folder: host.project_folder(), rate: s.sample_rate.hz(), bits: crate::audio::bits_of(s.bit_depth) });
        control
    }

    /// Remote mixing: the session (its pieces that changed) and the meters, while a PC mixes
    /// this one.
    pub fn publish_mix(&mut self, host: &dyn RigHost, hub: &Hub) {
        if hub.mixers().is_empty() {
            self.mix_rev = None;
            return;
        }
        let now = Instant::now();
        let rev = host.engine().revision;
        if self.mix_rev != Some(rev) && self.mix_at.is_none_or(|t| now.duration_since(t) >= MIX_EVERY) {
            match crate::mixer::pieces(host.engine().session()) {
                Ok(p) => hub.publish_mix(p),
                Err(e) => log::warn!("remote mixing: {e}"),
            }
            self.mix_rev = Some(rev);
            self.mix_at = Some(now);
        }
        if self.meters_at.is_none_or(|t| now.duration_since(t) >= METERS_EVERY) {
            let v = host.view();
            let mut f = host.mix_meters();
            f.playing = v.playing;
            f.recording = v.recording;
            f.position = v.position;
            hub.publish_meters(&f);
            self.meters_at = Some(now);
        }
    }

    /// A service remote button.
    pub fn action(&mut self, host: &mut dyn RigHost, action: RemoteAction) -> Result<Value, String> {
        let v = host.view();
        match action {
            RemoteAction::Record => {
                if !v.recording {
                    host.run("transport.record", json!({}))?;
                }
                Ok(json!({"recording": true}))
            }
            RemoteAction::Play => {
                if !v.playing {
                    host.run("transport.play", json!({}))?;
                }
                Ok(json!({"playing": true}))
            }
            RemoteAction::Stop { held } => {
                if v.recording && !held {
                    return Err("Recording: hold Stop to stop it".into());
                }
                if v.playing || v.recording {
                    host.run("transport.stop", json!({}))?;
                }
                Ok(json!({"stopped": true}))
            }
            RemoteAction::ArmAll { value } => {
                let tracks: Vec<u64> = armable(host.engine()).into_iter().map(|t| t.0).collect();
                if !tracks.is_empty() {
                    host.run("mix.record_arm", json!({"tracks": tracks, "value": value}))?;
                }
                Ok(json!({"tracks": tracks.len()}))
            }
            RemoteAction::Arm { track, value } => {
                if !armable(host.engine()).contains(&TrackId(track)) {
                    return Err("that track cannot record".into());
                }
                host.run("mix.record_arm", json!({"tracks": [track], "value": value}))
            }
            RemoteAction::Marker { name } => {
                let at = if v.playing || v.recording { v.position } else { host.engine().session().edit.selection.start };
                let name = name.map(|n| n.trim().chars().take(64).collect::<String>()).filter(|n| !n.is_empty());
                let name = name.unwrap_or_else(|| format!("Marker {}", clock_text(secs(host.engine(), at))));
                host.run("markers.add", json!({"name": name, "at": at}))
            }
            RemoteAction::RenameMarker { id, name } => {
                let name: String = name.trim().chars().take(64).collect();
                if name.is_empty() {
                    return Err("a marker needs a name".into());
                }
                host.run("markers.edit", json!({"id": id, "name": name}))
            }
        }
    }

    fn land(&mut self, host: &mut dyn RigHost, hub: &Hub, take: &TakeEnvelope, prepared: Option<&PreparedAudio>) -> Result<Value, String> {
        if hub.already_landed(&take.id) {
            return Ok(json!({"landed": take.id, "again": true}));
        }
        if let (Some(a), Some(p)) = (take.audio(), prepared) {
            let l = land_audio(host.engine_mut(), take, &p.wav, p.first)?;
            hub.note_landed(&take.pc, &take.id);
            let port = remote_port_name(&take.pc, &a.input);
            hub.clear_audio(Some(&take.pc));
            hub.add_clip_marks(&port, &p.clips);
            let mut said = format!("{}: take received ({}", take.pc, a.input);
            if p.converted {
                said.push_str(&format!(", converted from {} kHz", f64::from(a.rate) / 1000.0));
            }
            said.push(')');
            if take.partial {
                said.push_str(" (partial: it joined late)");
            }
            if !p.clips.is_empty() {
                let n = p.clips.len();
                said.push_str(&format!(". {n} clip{} in this take", if n == 1 { "" } else { "s" }));
            }
            host.notify(said);
            return Ok(json!({"landed": take.id, "clips": l.clips.iter().map(|c| c.0).collect::<Vec<_>>(), "clipped": p.clips.len()}));
        }
        let l = land(host.engine_mut(), take)?;
        hub.note_landed(&take.pc, &take.id);
        hub.clear_sketches(Some(&take.pc));
        let partial = if take.partial { " (partial: it joined late)" } else { "" };
        host.notify(format!("{}: take received, {} notes{partial}", take.pc, take.notes()));
        Ok(json!({"landed": take.id, "clips": l.clips.iter().map(|c| c.0).collect::<Vec<_>>()}))
    }

    fn transport(&mut self, host: &mut dyn RigHost, hub: &Hub) {
        let v = host.view();
        let r = rate(host.engine());
        let sec = |x: Samples| x as f64 / r;
        let state = state_of(&v);
        if state == TransportState::Recording && self.was != TransportState::Recording {
            self.take = new_take_id(self.take);
            self.anchor = Some(BoothAnchor {
                play_from: sec(v.play_from),
                record_start: sec(v.record_start),
                end: None,
                looped: v.looped.map(|l| [sec(l.start), sec(l.end)]),
                punch: v.punch.map(|p| [sec(p.start), sec(p.end)]),
            });
            self.record_began = Some(Instant::now());
            hub.clear_sketches(None);
            hub.clear_audio(None);
            let missing = hub.missing_followers();
            if !missing.is_empty() {
                host.notify(format!("{} not reachable: the booth records anyway", missing.join(", ")));
            }
        }
        if state == TransportState::Recording {
            self.last_pos = v.position;
            // The take starts where its first frame went (the host refines it once it has one).
            if let Some(a) = &mut self.anchor {
                a.record_start = sec(v.record_start);
                a.play_from = sec(v.play_from);
            }
        }
        if self.was == TransportState::Recording && state != TransportState::Recording {
            if let Some(a) = &mut self.anchor {
                a.end = Some(sec(self.last_pos));
            }
            self.record_began = None;
        }
        self.was = state;
        hub.set_position(sec(v.position));
        let session = host.engine().session().name.clone();
        let key = (state, self.take, self.anchor);
        if self.last_sent.as_ref() != Some(&key) {
            let looped = v.looped.filter(|_| v.playing).map(|l| [sec(l.start), sec(l.end)]);
            let (rate, bits) = (host.engine().session().sample_rate.hz(), crate::audio::bits_of(host.engine().session().bit_depth));
            hub.set_transport(BoothTransport { state, take: self.take, anchor: self.anchor, position: sec(v.position), looped, session, rate, bits });
            self.last_sent = Some(key);
            // A transport change goes to the page at once.
            self.last_publish = None;
        }
        let missing = hub.missing_followers();
        let warning = (state == TransportState::Recording && !missing.is_empty()).then(|| {
            let who = missing.join(", ");
            format!("{who} not reachable. The booth is recording; {who} will send its take when it reconnects.")
        });
        hub.set_warning(warning);
    }

    /// Publishes the status (at most every 66 ms unless `now`).
    pub fn publish(&mut self, host: &dyn RigHost, hub: &Hub, now: bool) {
        if !now && self.last_publish.is_some_and(|t| t.elapsed() < Duration::from_millis(66)) {
            return;
        }
        self.last_publish = Some(Instant::now());
        hub.publish(self.status(host, hub));
    }

    /// The status as the page shows it (the hub adds the remote recorders and browsers).
    pub fn status(&self, host: &dyn RigHost, hub: &Hub) -> RigStatus {
        let v = host.view();
        let e = host.engine();
        let s = e.session();
        let remote_of = |input: &Route| match input {
            Route::Midi(m) => m.device.as_deref().and_then(|d| d.strip_prefix("Remote: ")).and_then(|d| d.split(" › ").next()).map(str::to_string),
            _ => None,
        };
        let tracks: Vec<TrackStatus> = armable(e)
            .into_iter()
            .filter_map(|id| s.track(id))
            .map(|t| TrackStatus {
                id: t.id.0,
                name: t.name.clone(),
                armed: t.mixer.record_arm,
                level: host.level(t.id).clamp(0.0, 1.0),
                remote: remote_of(&t.mixer.input),
            })
            .collect();
        let markers = s.markers.iter().map(|m| MarkerStatus { id: m.id.0, number: m.number, name: m.name.clone(), at: secs(e, m.start) }).collect();
        let pos = if v.playing || v.recording { v.position } else { s.edit.selection.start };
        let tick = s.tempo.samples_to_ticks(pos, s.sample_rate);
        let bar = s.tempo.bar_beat_at_tick(tick).bar;
        let disk_free = host.disk_free();
        // 24-bit audio on each armed audio track.
        let armed_audio =
            s.tracks.iter().filter(|t| t.kind == TrackKind::Audio && t.mixer.record_arm).map(|t| t.format.channels().max(1) as u64).sum::<u64>();
        let per_hour = armed_audio as f64 * rate(e) * 3.0 * 3600.0;
        RigStatus {
            pc: hub.pc_name(),
            session: if s.name.is_empty() { "Untitled".into() } else { s.name.clone() },
            transport: state_of(&v),
            position: secs(e, pos),
            bar,
            record_elapsed: if v.recording { secs(e, v.position.saturating_sub(v.record_start)).max(0.0) } else { 0.0 },
            tracks,
            markers,
            disk_free,
            record_hours_left: disk_free.filter(|_| per_hour > 0.0).map(|f| f as f64 / per_hour),
            remotes: Vec::new(),
            clients: Vec::new(),
            warning: None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// A remote recorder

/// The local side of a take recorded for the booth.
#[derive(Debug, Clone)]
struct LocalTake {
    booth_take: u64,
    /// Captured once the host records: where its transport started, its loop, its rate.
    play_from: Option<Samples>,
    looped: Option<Range>,
    rate: f64,
}

#[derive(Debug, Default)]
pub struct FollowerGlue {
    current: Option<LocalTake>,
    /// Takes stopped, waiting for the host to hand over what it recorded.
    closing: VecDeque<(LocalTake, Option<crate::follower::ActiveTake>)>,
    feed_from: usize,
    notes: usize,
    /// The booth's session name (the local session is named after it).
    booth_session: String,
    /// Takes waiting to be sent at the last frame, and when the queue last emptied (a take
    /// landed on the booth): the badge says "Take sent" for a while.
    last_queued: usize,
    sent_at: Option<Instant>,
    /// The booth project's bit depth (the transfer's) and rate.
    booth_bits: u16,
    booth_rate: u32,
    /// Live audio blocks sent so far, per track; clips heard in this take (booth timeline s).
    audio_sent: Vec<(TrackId, usize)>,
    pub clips: Vec<f64>,
    /// The performer's last level check, for the booth's screen.
    pub check: Option<String>,
    /// The booth stopped at this timeline position (s); the take ends once the stream has
    /// played that far.
    stop_at: Option<(f64, Instant)>,
}

/// How long the badge says a take was sent.
const SENT_SHOWN: Duration = Duration::from_secs(10);

/// What a remote recorder's own screen says about following the booth (the toolbar badge): the
/// booth's name where it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowBadge {
    /// Recording a take for the booth.
    Recording(String),
    /// A take is on its way (how far, for audio).
    Sending(String, Option<u8>),
    /// An audio take is being made ready to send.
    Preparing(String),
    /// Takes stopped and kept, waiting for the booth (the count).
    Waiting(usize),
    /// The last take landed on the booth.
    Sent(String),
    /// Connected, nothing to do.
    Following(String),
    /// Not connected (looking for the booth).
    Away(String),
}

impl FollowBadge {
    pub fn text(&self) -> String {
        match self {
            FollowBadge::Recording(b) => format!("Recording with {b}"),
            FollowBadge::Sending(b, None) => format!("Sending take to {b}…"),
            FollowBadge::Sending(b, Some(p)) => format!("Sending take to {b} {p}%"),
            FollowBadge::Preparing(b) => format!("Preparing take for {b}…"),
            FollowBadge::Waiting(1) => "Waiting to send take…".to_string(),
            FollowBadge::Waiting(n) => format!("Waiting to send {n} takes…"),
            FollowBadge::Sent(b) => format!("Take sent to {b}"),
            FollowBadge::Following(b) => format!("Following {b}"),
            FollowBadge::Away(b) => format!("Looking for {b}…"),
        }
    }

    /// Recording (the record colour).
    pub fn recording(&self) -> bool {
        matches!(self, FollowBadge::Recording(_))
    }
}

/// The booth's name as this PC knows it: the one it is connected to, else the one it follows.
pub fn booth_name(f: &Follower) -> String {
    let pc = f.booth_pc();
    if !pc.is_empty() {
        return pc;
    }
    let t = f.target();
    if t.booth_name.is_empty() { "the booth".to_string() } else { t.booth_name }
}

impl FollowerGlue {
    /// One frame. `contact_timeout`: a take ends on its own after the booth has been away this long.
    pub fn step(&mut self, host: &mut dyn RigHost, f: &Follower, contact_timeout: Duration) {
        for ev in f.drain_events() {
            self.follow(host, f, &ev.transport, ev.at, ev.joined);
        }
        let v = host.view();
        // Where this take's clock started, once the host is recording.
        // A stop waiting for the stream to catch up (see `follow`).
        if let Some((end, since)) = self.stop_at
            && (f.stream_health().and_then(|h| h.heard_pos).is_none_or(|p| p >= end) || since.elapsed() >= Duration::from_secs(3))
        {
            self.stop_at = None;
            if self.current.is_some() {
                self.stop_local(host, f, Some(end));
            }
        }
        if let Some(c) = &mut self.current
            && v.recording
            && c.play_from.is_none()
        {
            c.play_from = Some(v.play_from);
            c.looped = v.looped;
            c.rate = rate(host.engine());
        }
        // The booth has been away too long: end the take here (it is sent when the booth is back).
        if self.current.is_some() && v.recording && f.since_contact().is_some_and(|d| d >= contact_timeout) && !f.connected() {
            host.notify("The booth has been away too long: the take ends here and is sent when it is back".into());
            self.stop_local(host, f, None);
        }
        let midi = host.take_finished();
        let audio = host.audio_finished();
        if midi.is_some() || audio.is_some() {
            self.finish(host, f, midi, audio);
        }
        self.live_audio(host, f);
        self.sketch(host, f);
        // The queue emptied: the last take landed on the booth.
        let queued = f.queued();
        if self.last_queued > 0 && queued == 0 && !f.sending() {
            self.sent_at = Some(Instant::now());
            host.notify(format!("Take sent to {}", booth_name(f)));
        }
        self.last_queued = queued;
        let state = if self.current.is_some() && v.recording {
            "recording"
        } else if f.sending() || f.progress().is_some() || f.preparing() {
            "transferring"
        } else if f.queued() > 0 {
            "waiting to transfer"
        } else {
            "connected"
        };
        f.set_report(FollowerReport {
            state: state.into(),
            notes: self.notes,
            disk_free: host.disk_free(),
            queued: f.queued(),
            ports: host.midi_ports(),
            inputs: host.audio_inputs(),
            clips: if self.current.is_some() { self.clips.len() } else { 0 },
            sending: f.progress(),
            check: self.check.clone(),
        });
    }

    fn follow(&mut self, host: &mut dyn RigHost, f: &Follower, t: &BoothTransport, at: Instant, joined: bool) {
        let v = host.view();
        if !t.session.is_empty() {
            self.booth_session = t.session.clone();
        }
        if t.bits > 0 {
            self.booth_bits = t.bits;
        }
        if t.rate > 0 {
            self.booth_rate = t.rate;
        }
        // Where the booth's playhead is now: when it was sent, plus the way here and the wait.
        let rolling = t.state != TransportState::Stopped;
        let now_pos = if rolling { t.position + f.half_rtt() + at.elapsed().as_secs_f64() } else { t.position };
        match t.state {
            TransportState::Recording => {
                if self.current.as_ref().is_some_and(|c| c.booth_take == t.take) {
                    return;
                }
                let Some(anchor) = t.anchor else { return };
                if v.recording || v.playing {
                    self.stop_local(host, f, None);
                }
                ensure_armed(host);
                // A take finished before this one (outside following) is not this one.
                if self.closing.is_empty() {
                    let _ = host.take_finished();
                }
                let e = host.engine_mut();
                let late = joined && now_pos - anchor.record_start > 1.0;
                let sel = match (anchor.looped, anchor.punch) {
                    (Some([s, en]), _) if !late => Range::new(samples(e, s), samples(e, en)),
                    (_, Some([s, en])) if !late => Range::new(samples(e, s), samples(e, en)),
                    _ => Range::point(samples(e, now_pos).max(0)),
                };
                let s = e.session_mut();
                s.edit.loop_record = anchor.looped.is_some() && !late;
                s.edit.selection = sel;
                // At once: the booth's take has started (its count-off, if any, is over).
                if let Err(err) = host.run("transport.record", json!({"countoff": false})) {
                    host.notify(format!("Following {}: cannot record: {err}", booth_name(f)));
                    return;
                }
                f.begin_take(t.take, anchor, late);
                self.current = Some(LocalTake { booth_take: t.take, play_from: None, looped: None, rate: rate(host.engine()) });
                self.feed_from = 0;
                self.notes = 0;
                self.audio_sent.clear();
                self.clips.clear();
                let mine = host.engine().session().sample_rate.hz();
                if self.booth_rate > 0 && mine != self.booth_rate && !host.audio_inputs().is_empty() {
                    host.notify(format!(
                        "{} records at {} kHz, this PC at {} kHz: the take is converted when it lands",
                        booth_name(f),
                        f64::from(self.booth_rate) / 1000.0,
                        f64::from(mine) / 1000.0
                    ));
                }
                let booth = booth_name(f);
                host.notify(if late { format!("Recording with {booth} (joined late)") } else { format!("Recording with {booth}") });
            }
            TransportState::Playing => {
                if v.recording {
                    self.stop_local(host, f, t.anchor.and_then(|a| a.end));
                }
                if !host.view().playing {
                    let e = host.engine_mut();
                    let at = samples(e, now_pos).max(0);
                    e.session_mut().edit.selection = Range::point(at);
                    let _ = host.run("transport.play", json!({}));
                }
            }
            TransportState::Stopped => {
                // Hearing the booth through the stream: he is still hearing the take's end; the
                // take stops where the booth stopped, once he has heard that far (or 3 s on).
                let end = t.anchor.and_then(|a| a.end).filter(|_| t.anchor.is_some_and(|a| a.looped.is_none()));
                if self.current.is_some()
                    && let Some(end) = end
                    && f.stream_health().and_then(|h| h.heard_pos).is_some_and(|p| p < end)
                {
                    self.stop_at = Some((end, Instant::now()));
                    f.set_end(Some(end));
                    return;
                }
                if self.current.is_some() {
                    self.stop_local(host, f, t.anchor.and_then(|a| a.end));
                } else if v.playing || v.recording {
                    let _ = host.run("transport.stop", json!({}));
                }
            }
        }
    }

    /// Stops the local take (the booth stopped at `end`, or is away).
    fn stop_local(&mut self, host: &mut dyn RigHost, f: &Follower, end: Option<f64>) {
        let v = host.view();
        if v.playing || v.recording {
            let _ = host.run("transport.stop", json!({}));
        }
        if let Some(c) = self.current.take() {
            f.set_end(end);
            self.closing.push_back((c, f.end_take()));
        }
    }

    /// The host handed over a finished local take (MIDI, audio or both): off it goes to the booth.
    fn finish(&mut self, host: &mut dyn RigHost, f: &Follower, midi: Option<FinishedTake>, audio: Option<FinishedAudio>) {
        // A take the host finished on its own (Stop pressed on this PC): it ends here.
        if self.closing.is_empty()
            && !host.view().recording
            && let Some(c) = self.current.take()
        {
            self.closing.push_back((c, f.end_take()));
        }
        let Some((local, active)) = self.closing.pop_front() else { return };
        let Some(active) = active else { return };
        let pc = f.target().my_name;
        let audio_takes = audio.as_ref().map_or(0, |a| a.takes.len());
        // The local session (the take's backup) is kept first: the take waiting to be sent is
        // kept in its project folder.
        let folder = host.keep_take(&self.booth_session);
        let mut said: Vec<String> = Vec::new();
        if let Some(a) = audio.filter(|a| !a.takes.is_empty()) {
            let rate = f64::from(a.rate.max(1));
            let play_from = local.play_from.unwrap_or(a.start);
            // The take clock counts from where the transport started (a loop's start when looping).
            let zero = local.looped.map_or(play_from, |l| l.start);
            let t0 = (a.start.saturating_sub(a.latency).saturating_sub(zero)) as f64 / rate;
            let bits = if self.booth_bits > 0 { self.booth_bits } else { crate::audio::bits_of(host.engine().session().bit_depth) };
            let (_, ext) = crate::audio::transfer_format(bits);
            for (n, (_, input, path)) in a.takes.iter().enumerate() {
                let id = format!("{pc}-{}-a{}", local.booth_take, n + 1);
                let env = TakeEnvelope {
                    id: id.clone(),
                    booth_take: local.booth_take,
                    pc: pc.clone(),
                    anchor: active.anchor,
                    sync: active.sync.clone(),
                    partial: active.partial,
                    payload: TakePayload::Audio(AudioTake {
                        heard: active.heard.clone(),
                        input: input.clone(),
                        rate: a.rate,
                        bits,
                        t0,
                        file: format!("{}.{ext}", crate::follower::file_safe(&id)),
                        source: path.to_string_lossy().into_owned(),
                        ..AudioTake::default()
                    }),
                };
                f.enqueue_in(env, folder.as_deref());
            }
            said.push(format!("{audio_takes} audio take{}", if audio_takes == 1 { "" } else { "s" }));
        }
        if let Some(done) = midi.filter(|m| !m.events.is_empty() || audio_takes == 0) {
            let notes = self.send_midi(f, &local, &active, done, folder.as_deref());
            said.push(format!("{notes} notes"));
        }
        self.last_queued = f.queued();
        self.sent_at = None;
        let booth = booth_name(f);
        let what = said.join(", ");
        host.notify(if f.connected() {
            format!("Take recorded ({what}): sending to {booth}")
        } else {
            format!("Waiting to send take… ({what}, {booth} away)")
        });
    }

    /// The MIDI part of a finished take, queued for the booth; its note count.
    fn send_midi(
        &mut self,
        f: &Follower,
        local: &LocalTake,
        active: &crate::follower::ActiveTake,
        done: FinishedTake,
        folder: Option<&std::path::Path>,
    ) -> usize {
        let play_from = local.play_from.unwrap_or(done.take.start);
        let rate = local.rate.max(1.0);
        let events: Vec<WireEvent> = done
            .events
            .iter()
            .map(|x| {
                let (at, pass) = played_at(&done.take, x.at, x.pass);
                let rolled = match local.looped.or(done.take.looped) {
                    Some(l) => i64::from(pass).saturating_mul(l.len()).saturating_add(at.saturating_sub(l.start)),
                    None => at.saturating_sub(play_from),
                };
                WireEvent { t: rolled as f64 / rate, port: x.port.clone(), msg: x.msg }
            })
            .collect();
        let pc = f.target().my_name;
        let env = TakeEnvelope {
            id: format!("{pc}-{}", local.booth_take),
            booth_take: local.booth_take,
            pc,
            anchor: active.anchor,
            sync: active.sync.clone(),
            partial: active.partial,
            payload: TakePayload::Midi { events },
        };
        let notes = env.notes();
        f.enqueue_in(env, folder);
        notes
    }

    /// The live level of the audio takes recording now, at the booth's time (best effort), and
    /// the clips heard.
    fn live_audio(&mut self, host: &mut dyn RigHost, f: &Follower) {
        let Some(c) = &self.current else { return };
        let v = host.view();
        if !v.recording {
            return;
        }
        let Some(play_from) = c.play_from else { return };
        let Some(active) = f.active_take() else { return };
        let looped = c.looped;
        let r = c.rate.max(1.0);
        for la in host.live_audio(&self.audio_sent) {
            let n = la.peaks.len();
            match self.audio_sent.iter_mut().find(|(t, _)| *t == la.track) {
                Some((_, sent)) => *sent = la.from + n,
                None => self.audio_sent.push((la.track, la.from + n)),
            }
            if n == 0 {
                continue;
            }
            // Blocks grouped to about 20 ms.
            let group = ((0.02 * r) as usize / la.block.max(1)).max(1);
            let zero = looped.map_or(play_from, |l| l.start);
            let at_block = |i: usize| -> f64 {
                let frame = (la.from + i) as i64 * la.block as i64;
                let timeline = la.start.saturating_sub(la.latency).saturating_add(frame);
                let rolled = timeline.saturating_sub(zero) as f64 / r;
                let booth = f.estimate_booth(rolled).unwrap_or(rolled);
                active.anchor.place(booth).0
            };
            let mut peaks: Vec<(f64, f32)> = Vec::new();
            let mut clips: Vec<f64> = Vec::new();
            for (g, chunk) in la.peaks.chunks(group).enumerate() {
                let peak = chunk.iter().copied().fold(0f32, f32::max);
                let at = at_block(g * group);
                peaks.push((at, peak));
                if peak >= crate::audio::CLIP_LEVEL && self.clips.last().is_none_or(|l| at - l > crate::audio::CLIP_GAP) {
                    self.clips.push(at);
                    clips.push(at);
                }
            }
            let level = peaks.last().map_or(0.0, |p| p.1);
            f.push_audio_feed(&la.input, &peaks, &clips, level);
        }
    }

    /// The live sketch: notes heard since the last frame, at the booth's time (best effort).
    fn sketch(&mut self, host: &mut dyn RigHost, f: &Follower) {
        let Some(c) = &self.current else { return };
        let v = host.view();
        if !v.recording {
            return;
        }
        let new = host.live_since(self.feed_from);
        if new.is_empty() {
            return;
        }
        self.feed_from += new.len();
        let Some(play_from) = c.play_from else { return };
        let Some(active) = f.active_take() else { return };
        let mut by_port: Vec<(String, Vec<SketchNote>)> = Vec::new();
        for x in new {
            let kind = x.msg[0] & 0xF0;
            if kind != 0x90 && kind != 0x80 {
                continue;
            }
            let vel = if kind == 0x80 { 0 } else { x.msg[2] };
            if vel > 0 {
                self.notes += 1;
            }
            let rolled = match c.looped {
                Some(l) => i64::from(x.pass).saturating_mul(l.len()).saturating_add(x.at.saturating_sub(l.start)),
                None => x.at.saturating_sub(play_from),
            };
            let local = rolled as f64 / c.rate.max(1.0);
            let booth = f.estimate_booth(local).unwrap_or(local);
            let (at, _) = active.anchor.place(booth);
            let note = SketchNote { at, pitch: x.msg[1], vel };
            match by_port.iter_mut().find(|(p, _)| *p == x.port) {
                Some((_, v)) => v.push(note),
                None => by_port.push((x.port.clone(), vec![note])),
            }
        }
        for (port, notes) in by_port {
            f.push_feed(&port, &notes);
        }
    }

    /// Recording for the booth now.
    pub fn recording(&self) -> bool {
        self.current.is_some()
    }

    /// The badge for this PC's screen.
    pub fn badge(&self, f: &Follower) -> FollowBadge {
        let booth = booth_name(f);
        if self.current.is_some() {
            FollowBadge::Recording(booth)
        } else if f.preparing() {
            FollowBadge::Preparing(booth)
        } else if f.sending() || f.progress().is_some() {
            FollowBadge::Sending(booth, f.progress().map(|p| (p * 100.0).clamp(0.0, 100.0) as u8))
        } else if f.queued() > 0 {
            FollowBadge::Waiting(f.queued())
        } else if self.sent_at.is_some_and(|t| t.elapsed() < SENT_SHOWN) {
            FollowBadge::Sent(booth)
        } else if f.connected() {
            FollowBadge::Following(booth)
        } else {
            FollowBadge::Away(booth)
        }
    }
}

/// An unattended remote recorder records its MIDI tracks: when none is armed, the MIDI tracks
/// on a hardware MIDI input are armed, and without any a "Piano" instrument track on all inputs
/// is made and armed.
fn ensure_armed(host: &mut dyn RigHost) {
    // An armed audio track (the guitar PC) or MIDI track: recorded as set up.
    if !armed_midi_tracks(host.engine().session()).is_empty()
        || host.engine().session().tracks.iter().any(|t| t.mixer.record_arm && t.kind == TrackKind::Audio)
    {
        return;
    }
    let on_midi: Vec<u64> = host
        .engine()
        .session()
        .tracks
        .iter()
        .filter(|t| t.kind.is_midi() && matches!(t.mixer.input, Route::Midi(_)) && !t.inactive)
        .map(|t| t.id.0)
        .collect();
    if !on_midi.is_empty() {
        let _ = host.run("mix.record_arm", json!({"tracks": on_midi, "value": true}));
        return;
    }
    let Ok(made) = host.run("track.new", json!({"kind": "instrument", "name": "Piano"})) else { return };
    if let Some(id) = made["tracks"].get(0).and_then(Value::as_u64) {
        let _ = host.run("track.input", json!({"tracks": [id], "input": "midi:all"}));
        let _ = host.run("mix.record_arm", json!({"tracks": [id], "value": true}));
    }
}
