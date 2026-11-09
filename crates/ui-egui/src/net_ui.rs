//! Setup › Network: the LAN remote (spec `docs/superpowers/specs/2026-10-31-lan-remote-recording-design.md`).
//!
//! - **Allow control from this network** (off by default): this PC listens on its port
//!   (default 48100) for the service remote page (`http://<this PC>:48100/`, with a QR code
//!   for the phone) and for paired PCs, and announces itself by name over mDNS. A device asking
//!   to pair gets a 4-digit code shown here on screen ([`pairing_prompt`]).
//! - **Follow a booth**: this PC becomes a remote recorder: it records, plays and stops with the
//!   booth it picked (found by name), and sends each take there.
//!
//! The network lives in `soundcraft_net` on its own threads; once a frame [`step`] runs its glue
//! against this app ([`AppHost`]). The take clock the network reads is the player's
//! [`soundcraft_playback::TransportProbe`] (atomics only).

use crate::SoundApp;
use crate::theme::Tokens;
use egui::{Color32, Sense, vec2};
use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_model::{Route, Session, Track, TrackId};
use soundcraft_net::config::NetConfig;
use soundcraft_net::discovery::Discovery;
use soundcraft_net::follower::{FollowTarget, Follower, pair_confirm, pair_request};
use soundcraft_net::glue::{BoothGlue, FinishedAudio, FinishedTake, FollowBadge, FollowerGlue, HostView, LiveAudio, RigHost};
use soundcraft_net::hub::{Hub, HubRequest, TakeClock};
use soundcraft_playback::TransportProbe;
use soundcraft_time::Samples;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

/// The take clock the network reads: the player's frames rolled since Play, plus the time since
/// that block, less the output latency and mix delay (the audible position), in seconds.
#[derive(Default)]
pub struct AppClock {
    probe: RwLock<Option<TransportProbe>>,
    rate: AtomicU32,
    latency: AtomicI64,
}

impl TakeClock for AppClock {
    fn now(&self) -> Option<f64> {
        let probe = self.probe.read().unwrap_or_else(PoisonError::into_inner);
        let r = probe.as_ref()?.read();
        if !r.playing {
            return None;
        }
        let rate = f64::from(self.rate.load(Ordering::Relaxed).max(1));
        // Since the block was rendered (at most a long block: a stalled device doesn't run on).
        let since = soundcraft_playback::clock_ns().saturating_sub(r.at_ns) as f64 * 1e-9;
        let rolled = r.rolled as f64 + since.min(0.1) * rate;
        Some((rolled - self.latency.load(Ordering::Relaxed) as f64) / rate)
    }
}

/// Pairing with a booth, on a thread: the request's id, then the token.
#[derive(Debug, Clone, Default)]
struct PairJob {
    request: Option<String>,
    result: Arc<Mutex<Option<Result<Value, String>>>>,
}

pub struct NetState {
    pub hub: Hub,
    discovery: Option<Discovery>,
    pub follower: Option<Follower>,
    booth: BoothGlue,
    follow: FollowerGlue,
    clock: Arc<AppClock>,
    qr: Option<(String, usize, Vec<bool>)>,
    outbox: Option<std::path::PathBuf>,
    /// A real app (with a settings file): sessions never saved are saved for remote takes.
    keeps_takes: bool,
    error: Option<String>,
    name_edit: Option<String>,
    addr_edit: String,
    code_edit: String,
    pair: PairJob,
    /// For renders (`follower_demo`): the badge shown instead of the follower's.
    demo_badge: Option<FollowBadge>,
    /// The performer's level window (`performer_panel`).
    guard: Guard,
    /// Over the internet: this PC's endpoint, the invite shown, the invite pasted.
    internet: Option<soundcraft_net::internet::Internet>,
    invite: Option<(String, std::time::Instant)>,
    invite_edit: String,
    join: PairJob,
    /// Mix this booth remotely (on) or stop (off): asked in the window, done once it is drawn.
    mix_request: Option<bool>,
    /// This PC is mixing the booth now (a copy of `SoundApp::remote_mix.is_some()` for the window).
    mixing: bool,
    /// Hear the booth's mix while mixing it: asked in the window.
    hear_request: Option<bool>,
    /// The tab shown, and what pairing or joining a booth says.
    pub tab: NetTab,
    connect_status: String,
    /// The invite shown is for a mixer (`mix`) or a performer; when it was copied.
    invite_role: String,
    copied: Option<std::time::Instant>,
    /// The QR code for the remote page is shown; the host is being changed (step 1 open again).
    show_qr: bool,
    change_host: bool,
    /// The X32 link (Setup › X32 Control) and its gestures (`x32_ui`).
    pub(crate) x32: Option<soundcraft_net::x32::X32Link>,
    pub(crate) x32_apply: soundcraft_net::x32::Apply,
}

impl NetState {
    fn new(app: &SoundApp, ctx: &egui::Context) -> NetState {
        let file = app.services.network_file.clone();
        let cfg = file.as_deref().map(NetConfig::load).unwrap_or_default();
        let dir = file.as_deref().and_then(std::path::Path::parent).map(std::path::Path::to_path_buf);
        let hub = Hub::new(cfg.clone(), file);
        let waker_ctx = ctx.clone();
        hub.set_waker(Some(Arc::new(move || waker_ctx.request_repaint())));
        let clock = Arc::new(AppClock::default());
        hub.set_clock(Some(clock.clone() as Arc<dyn TakeClock>));
        let mut n = NetState {
            hub,
            discovery: None,
            follower: None,
            booth: BoothGlue::default(),
            follow: FollowerGlue::default(),
            clock,
            qr: None,
            // Only pointers to the takes kept in their projects (and takes from before that).
            outbox: dir.as_ref().map(|d| d.join("Remote Outbox")),
            keeps_takes: dir.is_some(),
            error: None,
            name_edit: None,
            addr_edit: cfg.follow.booth_addr.clone(),
            code_edit: String::new(),
            pair: PairJob::default(),
            demo_badge: None,
            guard: Guard::default(),
            internet: None,
            invite: None,
            invite_edit: String::new(),
            join: PairJob::default(),
            mix_request: None,
            mixing: false,
            hear_request: None,
            // A PC that connects to a booth (and isn't one) opens on that tab.
            tab: if !cfg.follow.token.is_empty() && !cfg.enabled && !cfg.internet_host { NetTab::Connect } else { NetTab::Host },
            connect_status: String::new(),
            invite_role: String::new(),
            copied: None,
            show_qr: false,
            change_host: false,
            x32: None,
            x32_apply: soundcraft_net::x32::Apply::default(),
        };
        // Only a real app (with a settings file) opens the network at launch.
        if app.services.network_file.is_some() {
            if cfg.enabled {
                n.start_listening();
            }
            if cfg.internet_host {
                n.start_internet_host();
            }
            if cfg.follow.enabled && cfg.follow.at_launch {
                n.start_following(ctx);
            }
        }
        n
    }

    fn discovery(&mut self) -> Option<&mut Discovery> {
        if self.discovery.is_none() {
            match Discovery::new() {
                Ok(d) => self.discovery = Some(d),
                Err(e) => self.error = Some(e),
            }
        }
        self.discovery.as_mut()
    }

    fn start_listening(&mut self) {
        let cfg = self.hub.config();
        match self.hub.listen(cfg.port) {
            Ok(port) => {
                self.error = None;
                let name = self.hub.pc_name();
                let role = if cfg.follow.enabled { "recorder" } else { "booth" };
                if let Some(d) = self.discovery()
                    && let Err(e) = d.announce(&name, port, role)
                {
                    log::warn!("network: {e}");
                }
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn stop_listening(&mut self) {
        self.hub.stop();
        if let Some(d) = &mut self.discovery {
            d.withdraw();
        }
    }

    fn start_following(&mut self, ctx: &egui::Context) {
        if let Some(f) = self.follower.take() {
            f.stop();
        }
        let cfg = self.hub.config();
        // Over the internet: the follower talks to a local bridge to the booth.
        let over_internet = !cfg.follow.internet.is_empty();
        let addr = if over_internet {
            match self.bridge(&cfg.follow.internet) {
                Ok(a) => a,
                Err(e) => {
                    self.error = Some(e);
                    return;
                }
            }
        } else {
            cfg.follow.booth_addr.clone()
        };
        let target = FollowTarget { booth_name: cfg.follow.booth_name.clone(), addr, token: cfg.follow.token.clone(), my_name: self.hub.pc_name() };
        let f = Follower::start(target, self.outbox.clone(), Some(self.clock.clone() as Arc<dyn TakeClock>));
        let waker_ctx = ctx.clone();
        f.set_waker(Some(Arc::new(move || waker_ctx.request_repaint())));
        if over_internet {
            self.follower = Some(f);
            return;
        }
        if let Some(d) = self.discovery() {
            let _ = d.browse();
            let found = d.found_handle();
            f.set_lookup(found);
        }
        self.follower = Some(f);
    }

    /// This PC's internet endpoint (its key made once and kept in the network settings).
    fn internet(&mut self) -> Result<&soundcraft_net::internet::Internet, String> {
        use soundcraft_net::internet::{Internet, Relays, key_from_hex, key_hex, new_key};
        if self.internet.is_none() {
            let cfg = self.hub.config();
            let key = match key_from_hex(&cfg.internet_key) {
                Some(k) => k,
                None => {
                    let k = new_key();
                    self.hub.update_config(|c| c.internet_key = key_hex(&k));
                    k
                }
            };
            self.internet = Some(Internet::start(key, Relays::from_setting(&cfg.relay_url))?);
        }
        self.internet.as_ref().ok_or_else(|| "no internet endpoint".to_string())
    }

    /// Lets a performer join this PC with an invite: the hub's internet listener, bridged.
    fn start_internet_host(&mut self) {
        let port = match self.hub.listen_internet() {
            Ok(p) => p,
            Err(e) => {
                self.error = Some(e);
                return;
            }
        };
        match self.internet() {
            Ok(i) => i.serve_to(port),
            Err(e) => self.error = Some(e),
        }
    }

    /// A local port bridged to the booth whose address (JSON, from its invite) is `addr`.
    fn bridge(&mut self, addr: &str) -> Result<String, String> {
        Ok(self.internet()?.bridge_to_json(addr)?.to_string())
    }

    /// Makes an invite (valid 30 minutes, once): the code to send the performer.
    /// `role`: what the invited PC does (`""` records with this one, `mix` mixes it).
    fn make_invite(&mut self, role: &str) -> Result<String, String> {
        let addr = self.internet()?.addr(Duration::from_secs(5));
        let secret = self.hub.new_invite_secret();
        Ok(soundcraft_net::internet::Invite { addr, secret, name: self.hub.pc_name(), role: role.to_string() }.code())
    }

    fn stop_following(&mut self) {
        if let Some(f) = self.follower.take() {
            f.stop();
        }
    }

    /// Where a session never saved is saved (its own project folder in here) when it records a
    /// take for the booth: Setup › Network's choice, else `Documents/FreeDAWm/Remote Takes`.
    pub fn takes_dir(&self) -> Option<std::path::PathBuf> {
        if !self.keeps_takes {
            return None;
        }
        let chosen = self.hub.config().follow.takes_folder;
        if !chosen.is_empty() {
            return Some(chosen.into());
        }
        default_takes_dir()
    }

    /// The page's address (this PC's LAN address and port).
    fn url(&self) -> Option<String> {
        let port = self.hub.listening_port()?;
        let ip = soundcraft_net::sys::lan_ip().map_or_else(|| "localhost".to_string(), |i| i.to_string());
        Some(format!("http://{ip}:{port}/"))
    }
}

/// The app as the network glue sees it.
pub struct AppHost<'a>(pub &'a mut SoundApp);

impl RigHost for AppHost<'_> {
    fn engine(&self) -> &Engine {
        &self.0.engine
    }
    fn engine_mut(&mut self) -> &mut Engine {
        &mut self.0.engine
    }
    fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        self.0.run(id, params)
    }
    fn view(&self) -> HostView {
        let a = &*self.0;
        let recording = a.engine.transport.recording;
        HostView {
            playing: a.is_playing(),
            recording,
            position: a.position(),
            play_from: a.play_from,
            record_start: a.take_plan.as_ref().map_or(a.record_start, |p| p.start),
            looped: a.play_loop,
            punch: a.punch,
            counting_off: a.countoff_status().is_some(),
        }
    }
    fn mix_meters(&self) -> soundcraft_net::mixer::MeterFrame {
        use soundcraft_net::mixer::{Meter, MeterFrame};
        let a = &*self.0;
        let tracks = a.meters.iter().map(|(id, m)| Meter { id: id.0, l: m.level[0], r: m.level[1], gr: m.gr }).collect();
        let main = Meter { id: 0, l: a.main_meter.level[0], r: a.main_meter.level[1], gr: 0.0 };
        MeterFrame { tracks, main, ..MeterFrame::default() }
    }
    fn level(&self, track: TrackId) -> f32 {
        let m = self.0.meters.get(&track).map_or(0.0, |m| m.level[0].max(m.level[1]));
        if m <= 1e-6 { 0.0 } else { crate::widgets::meter_pos(20.0 * m.log10()) }
    }
    fn disk_free(&self) -> Option<u64> {
        let dir = self
            .0
            .engine
            .path
            .as_deref()
            .map(std::path::PathBuf::from)
            .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
            .unwrap_or_else(std::env::temp_dir);
        soundcraft_net::sys::disk_free(&dir)
    }
    fn take_finished(&mut self) -> Option<FinishedTake> {
        self.0.last_midi_take.take().map(|(take, events)| FinishedTake { take, events })
    }
    fn live_since(&self, from: usize) -> Vec<soundcraft_engine::midi_record::RecordedEvent> {
        let heard = self.0.midi_rec.get(from..).unwrap_or_default();
        crate::midi_devices::recorded_events(self.0, heard)
    }
    fn midi_ports(&self) -> Vec<String> {
        crate::midi_devices::present_ports(self.0)
    }
    fn notify(&mut self, text: String) {
        self.0.ui.status = text;
    }
    fn keep_take(&mut self, booth_session: &str) -> Option<std::path::PathBuf> {
        keep_session(self.0, booth_session)
    }
    fn audio_finished(&mut self) -> Option<FinishedAudio> {
        let (plan, takes) = self.0.last_audio_take.take()?;
        let s = self.0.engine.session();
        let takes = takes
            .into_iter()
            .map(|(id, path)| {
                let input = s.track(id).map_or_else(|| "In 1".to_string(), soundcraft_net::glue::audio_input_name);
                // The take's file as the session now names it (saved into the project folder).
                (id, input, path)
            })
            .collect();
        Some(FinishedAudio { start: plan.start, latency: plan.latency, rate: if plan.rate > 0 { plan.rate } else { s.sample_rate.hz() }, takes })
    }
    fn live_audio(&self, from: &[(TrackId, usize)]) -> Vec<LiveAudio> {
        let app = &*self.0;
        let Some(rec) = &app.recorder else { return Vec::new() };
        let (start, latency) = app.take_plan.as_ref().map_or((app.record_start, 0), |p| (p.start, p.latency));
        let s = app.engine.session();
        let mut out = Vec::new();
        for t in s.tracks.iter().filter(|t| t.mixer.record_arm && t.kind == soundcraft_model::TrackKind::Audio) {
            let Some(take) = rec.live_take(t.id) else { continue };
            let have = take.first().map_or(0, Vec::len);
            let begin = from.iter().find(|(id, _)| *id == t.id).map_or(0, |(_, n)| *n).min(have);
            let peaks: Vec<f32> =
                (begin..have).map(|i| take.iter().filter_map(|c| c.get(i)).map(|(lo, hi)| lo.abs().max(hi.abs())).fold(0f32, f32::max)).collect();
            out.push(LiveAudio {
                track: t.id,
                input: soundcraft_net::glue::audio_input_name(t),
                from: begin,
                peaks,
                block: soundcraft_playback::record::LIVE_BLOCK,
                start,
                latency,
            });
        }
        out
    }
}

/// A remote recorder keeps its local session (the take's backup) on disk: saved where it is, or,
/// never saved, as its own project folder "<PC> – <host session> – <date>" in the remote takes
/// folder (Setup › Network). Returns the session's project folder: the take waiting to be sent is
/// kept there too.
fn keep_session(app: &mut SoundApp, booth_session: &str) -> Option<std::path::PathBuf> {
    if app.engine.path.is_none() {
        let dir = app.net.as_ref().and_then(NetState::takes_dir)?;
        let pc = app.net.as_ref().map(|n| n.hub.pc_name()).unwrap_or_default();
        let booth = if booth_session.is_empty() { "Host" } else { booth_session };
        let name: String = format!("{pc} – {booth} – {}", today()).chars().map(|c| if r#"\/:*?"<>|"#.contains(c) { '-' } else { c }).collect();
        let name = name.trim().trim_end_matches('.').to_string();
        // Another run the same day gets its own folder.
        let path = (1..1000)
            .map(|n| if n == 1 { name.clone() } else { format!("{name} {n}") })
            .map(|n| dir.join(&n).join(format!("{n}.{}", soundcraft_engine::model::SESSION_EXTENSION)))
            .find(|p| p.parent().is_some_and(|d| !d.exists()))?;
        if let Err(e) = app.run("session.save", json!({"path": path.to_string_lossy()})) {
            log::warn!("remote take: could not save the session to {}: {e}", path.display());
            return None;
        }
    } else if let Err(e) = app.run("session.save", json!({})) {
        log::warn!("remote take: could not save the session: {e}");
    }
    app.engine.path.as_deref().and_then(|p| std::path::Path::new(p).parent()).map(std::path::Path::to_path_buf)
}

/// `Documents/FreeDAWm/Remote Takes` in the user's home folder.
fn default_takes_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|h| !h.is_empty())?;
    Some(std::path::PathBuf::from(home).join("Documents").join("FreeDAWm").join("Remote Takes"))
}

/// Today's date (UTC), `2026-11-01`.
fn today() -> String {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() / 86_400).unwrap_or(0) as i64;
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Once a frame (before the transport is handled): the network's requests and the glue.
pub fn step(app: &mut SoundApp, ctx: &egui::Context) {
    if app.net.is_none() {
        let n = NetState::new(app, ctx);
        app.net = Some(n);
    }
    let Some(mut net) = app.net.take() else { return };
    // The clock the network reads, and the mix stream's ends on the audio thread.
    let rate = app.engine.session().sample_rate.hz();
    net.hub.set_tap(app.player.as_ref().map(soundcraft_playback::Player::mix_tap), rate);
    if let Some(f) = &net.follower {
        let follow = net.hub.config().follow;
        let input = app.player.as_ref().map(soundcraft_playback::Player::stream_in);
        if let Some(i) = &input {
            i.set_gain(follow.mix_gain);
        }
        f.set_stream(follow.hear_mix, input, rate);
    }
    if let Some(p) = &app.player {
        *net.clock.probe.write().unwrap_or_else(PoisonError::into_inner) = Some(p.probe());
        net.clock.rate.store(app.engine.session().sample_rate.hz(), Ordering::Relaxed);
        net.clock.latency.store(crate::midi_devices::take_latency(app), Ordering::Relaxed);
    }
    if net.hub.listening_port().is_some() || net.hub.internet_port().is_some() {
        let control = net.booth.step(&mut AppHost(app), &net.hub);
        for r in control {
            if let HubRequest::Control { method, params, reply } = r {
                let req = crate::ControlRequest { method, params, reply };
                if let Some(v) = crate::control::handle(app, ctx, &req) {
                    let _ = req.reply.send(v);
                }
            }
        }
        // The status feed's clock and meters move while it rolls.
        ctx.request_repaint_after(Duration::from_millis(if app.is_playing() { 66 } else { 500 }));
    }
    match &net.follower {
        Some(f) => {
            let timeout = Duration::from_secs(net.hub.config().contact_timeout_s.max(1));
            net.follow.step(&mut AppHost(app), f, timeout);
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        None => {
            app.last_midi_take = None;
            app.last_audio_take = None;
        }
    }
    app.net = Some(net);
}

/// Pairing requests waiting: their codes, on screen, wherever the user is.
pub fn pairing_prompt(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(net) = &app.net else { return };
    let pending = net.hub.pending_pairings();
    if pending.is_empty() {
        return;
    }
    let t = Tokens::current();
    let mut deny = None;
    egui::Window::new("Pairing request").anchor(egui::Align2::RIGHT_TOP, vec2(-16.0, 60.0)).collapsible(false).resizable(false).show(ctx, |ui| {
        for p in &pending {
            ui.label(format!("{} wants to {}.", p.name, if p.kind == "pc" { "connect as a remote recorder" } else { "control this PC" }));
            ui.horizontal(|ui| {
                ui.label("Type this code on it:");
                ui.label(egui::RichText::new(&p.code).size(28.0).strong().monospace().color(t.text));
            });
            if ui.button("Deny").clicked() {
                deny = Some(p.id.clone());
            }
            ui.separator();
        }
        ui.label(egui::RichText::new("Nothing pairs without this code.").small().color(t.text_dim));
    });
    if let Some(id) = deny {
        net.hub.deny_pairing(&id);
    }
    ctx.request_repaint_after(Duration::from_millis(500));
}

/// Which half of Setup › Network is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NetTab {
    /// This PC is the host: who may connect, invites, who is connected.
    #[default]
    Host,
    /// This PC connects to a host: find it, then record with it or mix it.
    Connect,
}

/// What the window shows about this PC mixing the host (copied from the app before drawing).
#[derive(Debug, Clone, Default)]
struct MixShown {
    on: bool,
    connected: bool,
    hear: bool,
    status: String,
    rtt_ms: Option<f64>,
    delay_ms: Option<f64>,
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    pairing_prompt(app, ctx);
    performer_panel(app, ctx);
    mixing_banner(app, ctx);
    if let Some(on) = app.net.as_mut().and_then(|n| n.mix_request.take())
        && let Err(e) = remote_mix_command(app, &json!({"on": on}))
    {
        app.ui.status = e.clone();
        if let Some(n) = app.net.as_mut() {
            n.connect_status = e;
        }
    }
    if let Some(h) = app.net.as_mut().and_then(|n| n.hear_request.take())
        && let Some(rm) = app.remote_mix.as_mut()
    {
        rm.hear = h;
    }
    let shown = app.remote_mix.as_ref().map_or_else(MixShown::default, |r| MixShown {
        on: true,
        connected: r.client.connected(),
        hear: r.hear,
        status: r.client.status(),
        rtt_ms: r.client.rtt_ms(),
        delay_ms: r.client.delay_ms(),
    });
    if let Some(n) = app.net.as_mut() {
        n.mixing = shown.on;
    }
    let mut open = app.ui.show_network;
    if !open {
        return;
    }
    let Some(mut net) = app.net.take() else {
        app.ui.show_network = false;
        return;
    };
    let t = Tokens::current();
    egui::Window::new("Network")
        .open(&mut open)
        .default_size(vec2(700.0, 780.0))
        .min_width(600.0)
        .resizable(true)
        .frame(egui::Frame::window(&ctx.global_style()).fill(t.window_bg).inner_margin(egui::Margin::same(18)))
        .show(ctx, |ui| {
            header(ui, &mut net, &shown);
            ui.add_space(12.0);
            egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
                match net.tab {
                    NetTab::Host => host_tab(ui, &mut net, ctx),
                    NetTab::Connect => connect_tab(ui, &mut net, ctx, &shown, app.services.pick_folder.as_deref()),
                }
                ui.add_space(8.0);
            });
        });
    app.net = Some(net);
    app.ui.show_network = open;
}

/// This PC (its name, editable, and a sentence on what it is doing), and Host / Connect.
fn header(ui: &mut egui::Ui, net: &mut NetState, mix: &MixShown) {
    use crate::net_look::{Icon, avatar, segmented};
    let t = Tokens::current();
    let cfg = net.hub.config();
    let lan = net.hub.listening_port().is_some();
    let others = net.hub.clients().len() + net.hub.remotes().iter().filter(|r| r.state != "not reachable").count() + net.hub.mixers().len();
    let status = if mix.on {
        format!("Mixing {}", if cfg.follow.booth_name.is_empty() { "a host" } else { &cfg.follow.booth_name })
    } else if net.follower.is_some() {
        format!("Recording with {}", if cfg.follow.booth_name.is_empty() { "a host" } else { &cfg.follow.booth_name })
    } else {
        let doors = match (lan, cfg.internet_host) {
            (true, true) => "Open on this network and the internet",
            (true, false) => "Open on this network",
            (false, true) => "Open to invites over the internet",
            (false, false) => "Private: nothing can connect",
        };
        if others > 0 { format!("{doors} · {others} connected") } else { doors.to_string() }
    };
    ui.horizontal(|ui| {
        let busy = mix.on || net.follower.is_some() || others > 0;
        avatar(ui, 46.0, Icon::Pc, if busy || lan || cfg.internet_host { t.accent } else { t.button }, Color32::WHITE);
        ui.add_space(6.0);
        ui.vertical(|ui| {
            let mut name = net.name_edit.clone().unwrap_or_else(|| net.hub.pc_name());
            let r = ui.add(
                egui::TextEdit::singleline(&mut name)
                    .font(egui::FontId::proportional(20.0))
                    .frame(egui::Frame::NONE)
                    .desired_width(240.0)
                    .hint_text("This PC's name"),
            );
            let r = r.on_hover_text("This PC's name on the network: click to change it");
            if r.changed() {
                net.name_edit = Some(name.clone());
            }
            if r.lost_focus() && net.name_edit.is_some() {
                let n = name.trim().to_string();
                net.hub.update_config(|c| c.pc_name = n);
                net.name_edit = None;
                if net.hub.listening_port().is_some() {
                    net.start_listening();
                }
            }
            ui.label(egui::RichText::new(status).size(12.5).color(t.text_dim));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let selected = usize::from(net.tab == NetTab::Connect);
            if let Some(i) = segmented(ui, ["This PC is the host", "Connect to a host"], selected) {
                net.tab = if i == 0 { NetTab::Host } else { NetTab::Connect };
            }
        });
    });
    if let Some(e) = net.error.clone() {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.colored_label(t.rec, &e);
            if ui.small_button("✕").clicked() {
                net.error = None;
            }
        });
    }
}

// ---------------------------------------------------------------------------------------------
// This PC is the host

fn host_tab(ui: &mut egui::Ui, net: &mut NetState, ctx: &egui::Context) {
    use crate::net_look::{Icon, primary, secondary, section, switch, tile, tile_head};
    let t = Tokens::current();
    let cfg = net.hub.config();
    section(ui, "Who can connect");
    ui.columns(2, |cols| {
        // This network.
        let lan = cfg.enabled;
        tile(&mut cols[0], lan, |ui| {
            let mut on = lan;
            tile_head(ui, Icon::Wifi, lan, "This network", "Phones, the stage PC and other PCs on the same Wi-Fi or cable", |ui| {
                if switch(ui, &mut on, "On this network").changed() {
                    net.hub.update_config(|c| c.enabled = on);
                    if on { net.start_listening() } else { net.stop_listening() }
                }
            });
            if let Some(url) = net.url() {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    crate::net_look::pill(ui, url.trim_start_matches("http://").trim_end_matches('/'), t.text, t.button);
                    let label = if net.show_qr { "Hide QR code" } else { "QR code for a phone" };
                    if ui.small_button(label).clicked() {
                        net.show_qr = !net.show_qr;
                    }
                });
                if net.show_qr {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if net.qr.as_ref().is_none_or(|(u, _, _)| *u != url) {
                            net.qr = soundcraft_net::sys::qr_matrix(&url).map(|(w, m)| (url.clone(), w, m));
                        }
                        if let Some((_, w, m)) = &net.qr {
                            draw_qr(ui, *w, m, 120.0);
                        }
                        ui.label(
                            egui::RichText::new("Scan with the phone's camera for the remote page. Its first visit asks for a code shown here.")
                                .size(11.5)
                                .color(t.text_dim),
                        );
                    });
                }
            } else {
                ui.add_space(8.0);
                ui.label(egui::RichText::new("Off").size(11.5).color(t.text_dim));
            }
        });
        // The internet.
        let host = cfg.internet_host;
        tile(&mut cols[1], host, |ui| {
            let mut on = host;
            tile_head(ui, Icon::Globe, host, "The internet", "Anyone you send an invite to. Encrypted, direct when the routers allow", |ui| {
                if switch(ui, &mut on, "Over the internet").changed() {
                    net.hub.update_config(|c| c.internet_host = on);
                    if on {
                        net.start_internet_host();
                    } else {
                        net.hub.stop_internet();
                        net.invite = None;
                    }
                }
            });
            ui.add_space(8.0);
            if host {
                ui.horizontal(|ui| {
                    for (label, role, main) in [("Invite a mixer", soundcraft_net::internet::ROLE_MIX, true), ("Invite a performer", "", false)] {
                        let r = if main { primary(ui, label) } else { secondary(ui, label) };
                        let r = r.on_hover_text(if main {
                            "They mix this session from their PC and hear its mix"
                        } else {
                            "They record at home against this PC's mix; their take lands here"
                        });
                        if r.clicked() {
                            match net.make_invite(role) {
                                Ok(code) => {
                                    net.invite = Some((code, std::time::Instant::now()));
                                    net.invite_role = role.to_string();
                                }
                                Err(e) => net.error = Some(e),
                            }
                        }
                    }
                });
            } else {
                ui.label(egui::RichText::new("Off").size(11.5).color(t.text_dim));
            }
        });
    });
    if let Some((code, made)) = net.invite.clone()
        && cfg.internet_host
    {
        ui.add_space(10.0);
        invite_card(ui, net, ctx, &code, made);
    }
    section(ui, "Connected now");
    let nodes = connected_nodes(net);
    let me = net.hub.pc_name();
    crate::net_look::map(ui, &me, &nodes, "Nobody yet. Open a door above, then pair or invite someone.");
    internet_warning(ui, net);
    section(ui, "Trusted devices");
    trusted(ui, net);
    advanced(ui, net, ctx);
}

/// The invite just made: who it is for, the code, Copy, and how long it lasts.
fn invite_card(ui: &mut egui::Ui, net: &mut NetState, ctx: &egui::Context, code: &str, made: std::time::Instant) {
    use crate::net_look::{pill, primary, tile};
    let t = Tokens::current();
    let mix = net.invite_role == soundcraft_net::internet::ROLE_MIX;
    let left = (30.0 - made.elapsed().as_secs_f64() / 60.0).ceil().max(0.0);
    tile(ui, true, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(if mix { "Invite for a mixer" } else { "Invite for a performer" }).size(15.0).strong().color(t.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Discard").clicked() {
                    net.invite = None;
                }
                pill(ui, &format!("expires in {left:.0} min · works once"), t.text_dim, t.button);
            });
        });
        ui.add_space(6.0);
        let short = if code.len() > 64 { format!("{}…{}", &code[..40], &code[code.len() - 16..]) } else { code.to_string() };
        egui::Frame::new().fill(t.window_bg).corner_radius(7.0).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(short).monospace().size(12.0).color(t.text)).on_hover_text(code);
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if primary(ui, "Copy invite").clicked() {
                ctx.copy_text(code.to_string());
                net.copied = Some(std::time::Instant::now());
            }
            let copied = net.copied.is_some_and(|c| c.elapsed() < Duration::from_secs(3));
            ui.label(
                egui::RichText::new(if copied {
                    "Copied. Send it by email or chat."
                } else {
                    "Send it by email or chat. They paste it in Network › Connect to a host."
                })
                .size(11.5)
                .color(if copied { crate::net_look::LAN } else { t.text_dim }),
            );
        });
    });
}

/// Everyone connected to this PC, for the map.
fn connected_nodes(net: &NetState) -> Vec<crate::net_look::Node> {
    use crate::net_look::{Icon, Link, Node};
    let quality = net.internet.as_ref().map(soundcraft_net::internet::Internet::quality).unwrap_or_default();
    // The internet links are not named per device: the first one's quality stands for them.
    let inet = |rtt: Option<f64>| match quality.first() {
        Some(q) => Link::Internet { direct: q.direct, rtt_ms: Some(q.rtt_ms) },
        None => Link::Internet { direct: true, rtt_ms: rtt },
    };
    let mut v = Vec::new();
    for c in net.hub.clients() {
        v.push(Node { name: c, role: "remote page".into(), icon: Icon::Phone, link: Link::Lan, warn: false });
    }
    let internet_names: Vec<String> = net.hub.config().paired.iter().filter(|_| net.hub.internet_port().is_some()).map(|d| d.name.clone()).collect();
    for r in net.hub.remotes().into_iter().filter(|r| r.state != "not reachable") {
        let over = net.hub.internet_port().is_some() && internet_names.contains(&r.name) && !quality.is_empty();
        v.push(Node {
            name: r.name.clone(),
            role: format!("records · {}", r.state),
            icon: Icon::Mic,
            link: if over { inet(None) } else { Link::Lan },
            warn: r.warning,
        });
    }
    for m in net.hub.mixer_links() {
        v.push(Node {
            name: m.name.clone(),
            role: if m.delay_ms > 0.0 { format!("mixing · {:.0} ms behind", m.delay_ms) } else { "mixing".into() },
            icon: Icon::Faders,
            link: if m.internet { inet(None) } else { Link::Lan },
            warn: false,
        });
    }
    v
}

/// A weak internet link, in plain words.
fn internet_warning(ui: &mut egui::Ui, net: &NetState) {
    let Some(inet) = &net.internet else { return };
    if inet.quality().iter().any(soundcraft_net::internet::Quality::weak) {
        ui.add_space(6.0);
        ui.colored_label(
            crate::net_look::RELAY,
            "The internet link is weak: the mix may drop out. Recorded takes are safe: they are recorded on the remote PC.",
        );
    }
}

/// The devices paired with this PC, with Forget.
fn trusted(ui: &mut egui::Ui, net: &mut NetState) {
    use crate::net_look::{Icon, avatar};
    let t = Tokens::current();
    let cfg = net.hub.config();
    if cfg.paired.is_empty() {
        ui.label(egui::RichText::new("None yet: a device you pair or invite stays trusted until you forget it.").size(12.0).color(t.text_dim));
        return;
    }
    let mut forget = None;
    egui::Frame::new().fill(t.panel_bg2).corner_radius(10.0).inner_margin(egui::Margin::symmetric(12, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        for (i, d) in cfg.paired.iter().enumerate() {
            if i > 0 {
                ui.separator();
            }
            ui.horizontal(|ui| {
                let ic = if d.kind == "browser" { Icon::Phone } else { Icon::Pc };
                avatar(ui, 26.0, ic, t.button, t.text_dim);
                ui.label(egui::RichText::new(&d.name).color(t.text));
                ui.label(
                    egui::RichText::new(if d.kind == "browser" {
                        "phone or browser"
                    } else if d.follows {
                        "PC · has recorded with this one"
                    } else {
                        "PC"
                    })
                    .size(11.5)
                    .color(t.text_dim),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Forget").on_hover_text("It has to pair again to connect").clicked() {
                        forget = Some(d.token.clone());
                    }
                });
            });
        }
    });
    if let Some(tok) = forget {
        net.hub.forget(&tok);
    }
}

/// The port and the relay.
fn advanced(ui: &mut egui::Ui, net: &mut NetState, ctx: &egui::Context) {
    let t = Tokens::current();
    let cfg = net.hub.config();
    ui.add_space(10.0);
    egui::CollapsingHeader::new(egui::RichText::new("Advanced").color(t.text_dim)).id_salt("net-advanced").show(ui, |ui| {
        egui::Grid::new("net-advanced-grid").num_columns(2).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
            ui.label("Port");
            ui.horizontal(|ui| {
                let mut port = i64::from(cfg.port);
                if ui.add(egui::DragValue::new(&mut port).range(1024..=65535)).changed() {
                    let p = u16::try_from(port).unwrap_or(soundcraft_net::config::DEFAULT_PORT);
                    net.hub.update_config(|c| c.port = p);
                    if net.hub.listening_port().is_some() {
                        net.start_listening();
                    }
                }
                ui.label(egui::RichText::new("on this network (the remote page and the PCs)").size(11.5).color(t.text_dim));
            });
            ui.end_row();
            ui.label("Internet relay");
            ui.vertical(|ui| {
                let mut relay = cfg.relay_url.clone();
                let r = ui.add(egui::TextEdit::singleline(&mut relay).hint_text("the free public relays").desired_width(280.0));
                if r.changed() {
                    net.hub.update_config(|c| c.relay_url = relay.trim().to_string());
                }
                if r.lost_focus() && net.internet.is_some() {
                    // Starts again on the new relay.
                    net.internet = None;
                    if net.hub.config().internet_host {
                        net.start_internet_host();
                    }
                    if net.follower.is_some() && !net.hub.config().follow.internet.is_empty() {
                        net.start_following(ctx);
                    }
                }
                ui.label(
                    egui::RichText::new(
                        "Only used when two routers block a direct connection. Empty: n0's free public relays. Your own iroh-relay: its address.",
                    )
                    .size(11.0)
                    .color(t.text_dim),
                );
            });
            ui.end_row();
        });
    });
}

// ---------------------------------------------------------------------------------------------
// Connect to a host

fn connect_tab(ui: &mut egui::Ui, net: &mut NetState, ctx: &egui::Context, mix: &MixShown, pick_folder: Option<&dyn Fn(&str) -> Option<String>>) {
    use crate::net_look::{Icon, avatar, step, tile};
    let t = Tokens::current();
    take_join_result(net, ctx);
    let busy = net.follower.is_some() || mix.on;
    let cfg = net.hub.config();
    let paired = !cfg.follow.token.is_empty();
    ui.add_space(4.0);
    step(ui, 1, paired && !net.change_host, "Find the host");
    if paired && !net.change_host {
        tile(ui, false, |ui| {
            ui.horizontal(|ui| {
                avatar(ui, 40.0, Icon::Pc, t.accent, Color32::WHITE);
                ui.add_space(4.0);
                ui.vertical(|ui| {
                    let name = if cfg.follow.booth_name.is_empty() { cfg.follow.booth_addr.clone() } else { cfg.follow.booth_name.clone() };
                    ui.label(egui::RichText::new(name).size(17.0).strong().color(t.text));
                    let (how, col) = if cfg.follow.internet.is_empty() {
                        (format!("On this network · {}", cfg.follow.booth_addr), crate::net_look::LAN)
                    } else {
                        ("Over the internet · paired with an invite".to_string(), crate::net_look::NET)
                    };
                    ui.label(egui::RichText::new(how).size(12.0).color(col));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(!busy, egui::Button::new("Change host").corner_radius(7.0))
                        .on_disabled_hover_text("Stop recording or mixing first")
                        .clicked()
                    {
                        net.change_host = true;
                    }
                });
            });
        });
    } else {
        ui.columns(2, |cols| {
            tile(&mut cols[0], false, |ui| {
                crate::net_look::tile_head(ui, Icon::Wifi, true, "On this network", "Pair with the 4-digit code the host shows", |_| {});
                ui.add_space(8.0);
                find_on_lan(ui, net, ctx);
            });
            tile(&mut cols[1], false, |ui| {
                crate::net_look::tile_head(ui, Icon::Globe, true, "Over the internet", "Paste the invite the host sent you", |_| {});
                ui.add_space(8.0);
                join_with_invite(ui, net, ctx);
            });
        });
        if paired && ui.small_button("Keep the host I had").clicked() {
            net.change_host = false;
        }
    }
    if !net.connect_status.is_empty() {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(&net.connect_status).size(12.5).color(t.text_dim));
    }
    ui.add_space(14.0);
    let ready = paired && !net.change_host;
    step(ui, 2, busy, "Choose what this PC does");
    ui.add_enabled_ui(ready, |ui| {
        ui.columns(2, |cols| {
            record_choice(&mut cols[0], net, ctx, mix.on);
            mix_choice(&mut cols[1], net, mix);
        });
        if net.follower.is_some() || !mix.on {
            recording_options(ui, net, pick_folder);
        }
    });
    internet_warning(ui, net);
    advanced(ui, net, ctx);
}

/// The hosts found on this network (or an address), and pairing with its code.
fn find_on_lan(ui: &mut egui::Ui, net: &mut NetState, ctx: &egui::Context) {
    use crate::net_look::primary;
    let t = Tokens::current();
    let cfg = net.hub.config();
    let found = net.discovery.as_ref().map(Discovery::found).unwrap_or_default();
    let me = net.hub.pc_name();
    let pick = |net: &mut NetState, name: String, addr: String| {
        net.addr_edit = addr.clone();
        net.hub.update_config(|c| {
            if c.follow.booth_addr != addr || !c.follow.internet.is_empty() {
                c.follow.token.clear();
            }
            c.follow.booth_name = name;
            c.follow.booth_addr = addr;
            c.follow.internet.clear();
        });
    };
    let others: Vec<_> = found.iter().filter(|f| f.name != me).cloned().collect();
    if others.is_empty() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(egui::RichText::new("Looking for hosts…").size(12.0).color(t.text_dim));
        });
    }
    for f in &others {
        let on = cfg.follow.internet.is_empty() && cfg.follow.booth_addr == f.addr;
        let r = ui.add(egui::Button::selectable(on, format!("{}   {}", f.name, f.addr)).min_size(vec2(ui.available_width(), 28.0)));
        if r.clicked() {
            pick(net, f.name.clone(), f.addr.clone());
        }
    }
    ui.horizontal(|ui| {
        if ui.small_button("Search again").clicked()
            && let Some(d) = net.discovery()
        {
            let _ = d.browse();
        }
        ui.label(egui::RichText::new("or").size(11.5).color(t.text_dim));
        let r = ui.add(egui::TextEdit::singleline(&mut net.addr_edit).hint_text("address, e.g. 192.168.1.20").desired_width(150.0));
        if r.lost_focus() {
            let mut a = net.addr_edit.trim().to_string();
            if !a.is_empty() && !a.contains(':') {
                a = format!("{a}:{}", soundcraft_net::config::DEFAULT_PORT);
            }
            if a != cfg.follow.booth_addr {
                pick(net, String::new(), a);
            }
        }
    });
    ui.add_space(6.0);
    let addr = net.hub.config().follow.booth_addr;
    if let Some(req) = net.pair.request.clone() {
        let host = if cfg.follow.booth_name.is_empty() { "the host".to_string() } else { cfg.follow.booth_name.clone() };
        ui.label(egui::RichText::new(format!("Type the code {host} shows")).size(12.5).color(t.text));
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut net.code_edit)
                    .font(egui::FontId::monospace(20.0))
                    .desired_width(80.0)
                    .hint_text("0000")
                    .char_limit(4),
            );
            if primary(ui, "Pair").clicked() {
                let job = Arc::clone(&net.pair.result);
                let (addr, code, c) = (addr.clone(), net.code_edit.trim().to_string(), ctx.clone());
                net.code_edit.clear();
                std::thread::spawn(move || {
                    let r = pair_confirm(&addr, &req, &code).map(|(token, pc)| json!({"token": token, "pc": pc}));
                    *job.lock().unwrap_or_else(PoisonError::into_inner) = Some(r);
                    c.request_repaint();
                });
            }
        });
    } else if ui.add_enabled_ui(!addr.is_empty(), |ui| primary(ui, "Pair")).inner.on_disabled_hover_text("Pick a host or type its address").clicked()
    {
        let job = Arc::clone(&net.pair.result);
        *job.lock().unwrap_or_else(PoisonError::into_inner) = None;
        net.connect_status = "Asking the host for a code…".into();
        let me = me.clone();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let r = pair_request(&addr, &me).map(|id| json!({"request": id}));
            *job.lock().unwrap_or_else(PoisonError::into_inner) = Some(r);
            c.request_repaint();
        });
    }
    let got = net.pair.result.lock().unwrap_or_else(PoisonError::into_inner).take();
    if let Some(r) = got {
        match r {
            Ok(v) if v.get("request").is_some() => {
                net.pair.request = v["request"].as_str().map(str::to_string);
                net.connect_status.clear();
            }
            Ok(v) => {
                let token = v["token"].as_str().unwrap_or("").to_string();
                let host = v["pc"].as_str().unwrap_or("").to_string();
                net.hub.update_config(|c| {
                    c.follow.token = token;
                    c.follow.internet.clear();
                    if !host.is_empty() {
                        c.follow.booth_name = host;
                    }
                });
                net.pair.request = None;
                net.change_host = false;
                net.connect_status = "Paired. Now choose what this PC does.".into();
            }
            Err(e) => {
                net.pair.request = None;
                net.connect_status = e;
            }
        }
    }
}

/// Paste an invite and join: this PC pairs with the host across the internet.
fn join_with_invite(ui: &mut egui::Ui, net: &mut NetState, ctx: &egui::Context) {
    use crate::net_look::primary;
    ui.add(
        egui::TextEdit::multiline(&mut net.invite_edit)
            .hint_text("freedawm-invite:…")
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .font(egui::TextStyle::Monospace),
    );
    ui.add_space(6.0);
    let ready = !net.invite_edit.trim().is_empty();
    if ui.add_enabled_ui(ready, |ui| primary(ui, "Join")).inner.clicked() {
        match soundcraft_net::internet::Invite::parse(&net.invite_edit) {
            Err(e) => net.connect_status = e,
            Ok(inv) => {
                let addr_json = serde_json::to_string(&inv.addr).unwrap_or_default();
                match net.bridge(&addr_json) {
                    Err(e) => net.connect_status = e,
                    Ok(bridge) => {
                        net.connect_status = format!("Joining {}…", inv.name);
                        let job = Arc::clone(&net.join.result);
                        let me = net.hub.pc_name();
                        let c = ctx.clone();
                        let host = inv.name.clone();
                        let role = inv.role.clone();
                        std::thread::spawn(move || {
                            let r = soundcraft_net::follower::pair_invite(&bridge, &inv.secret, &me).map(
                                |(token, pc)| json!({"token": token, "pc": if pc.is_empty() { host } else { pc }, "addr": addr_json, "role": role}),
                            );
                            *job.lock().unwrap_or_else(PoisonError::into_inner) = Some(r);
                            c.request_repaint();
                        });
                    }
                }
            }
        }
    }
}

/// An invite accepted: paired across the internet; a performer's invite starts recording with
/// the host (hearing its mix), a mixer's starts mixing it.
fn take_join_result(net: &mut NetState, ctx: &egui::Context) {
    let got = net.join.result.lock().unwrap_or_else(PoisonError::into_inner).take();
    let Some(r) = got else { return };
    match r {
        Ok(v) => {
            let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
            let (token, pc, addr, role) = (s("token"), s("pc"), s("addr"), s("role"));
            let mix = role == soundcraft_net::internet::ROLE_MIX;
            net.hub.update_config(|c| {
                c.follow.token = token;
                c.follow.booth_name = pc.clone();
                c.follow.internet = addr;
                if !mix {
                    c.follow.enabled = true;
                    c.follow.hear_mix = true;
                }
            });
            net.invite_edit.clear();
            net.change_host = false;
            if mix {
                net.connect_status = format!("Joined {pc}: mixing it.");
                net.mix_request = Some(true);
            } else {
                net.connect_status = format!("Joined {pc}: recording with it and hearing its mix.");
                net.start_following(ctx);
            }
        }
        Err(e) => net.connect_status = e,
    }
}

/// The choice "Record with it": a tile that starts and stops it.
fn record_choice(ui: &mut egui::Ui, net: &mut NetState, ctx: &egui::Context, mixing: bool) {
    use crate::net_look::{Icon, tile, tile_head};
    let t = Tokens::current();
    let on = net.follower.is_some();
    tile(ui, on, |ui| {
        tile_head(ui, Icon::Mic, on, "Record with it", "This PC records, plays and stops with the host; each take lands there", |_| {});
        ui.add_space(10.0);
        if let Some(f) = &net.follower {
            let badge = net.follow.badge(f);
            let detail = if f.connected() { String::new() } else { format!(" ({})", f.state()) };
            ui.colored_label(if badge.recording() { t.rec } else { crate::net_look::LAN }, format!("● {}{detail}", badge.text()));
            ui.add_space(6.0);
        }
        let r = if on {
            crate::net_look::secondary(ui, "Stop recording with it")
        } else {
            ui.add_enabled_ui(!mixing, |ui| crate::net_look::primary(ui, "Start")).inner.on_disabled_hover_text("Stop mixing first")
        };
        if r.clicked() {
            net.hub.update_config(|c| c.follow.enabled = !on);
            if on { net.stop_following() } else { net.start_following(ctx) }
        }
    });
}

/// The choice "Mix it": a tile that starts and stops it, with Hear and the link.
fn mix_choice(ui: &mut egui::Ui, net: &mut NetState, mix: &MixShown) {
    use crate::net_look::{Icon, switch, tile, tile_head};
    let t = Tokens::current();
    tile(ui, mix.on, |ui| {
        tile_head(
            ui,
            Icon::Faders,
            mix.on,
            "Mix it",
            "Its session in your Mix window, its mix in your speakers. Mix and transport go to the host",
            |_| {},
        );
        ui.add_space(10.0);
        if mix.on {
            if mix.connected {
                let mut line = "● Connected".to_string();
                if let Some(r) = mix.rtt_ms {
                    line.push_str(&format!(" · {r:.0} ms round trip"));
                }
                ui.colored_label(crate::net_look::LAN, line);
                if let Some(d) = mix.delay_ms {
                    ui.label(
                        egui::RichText::new(format!("You hear it {d:.0} ms late; your automation lands where you heard it."))
                            .size(11.5)
                            .color(t.text_dim),
                    );
                }
            } else {
                ui.colored_label(crate::net_look::RELAY, format!("● {}", mix.status));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let mut hear = mix.hear;
                if switch(ui, &mut hear, "Hear its mix").changed() {
                    net.hear_request = Some(hear);
                }
                ui.label("Hear its mix");
            });
            ui.add_space(6.0);
            if crate::net_look::secondary(ui, "Stop mixing").clicked() {
                net.mix_request = Some(false);
            }
        } else {
            let r = ui
                .add_enabled_ui(net.follower.is_none(), |ui| crate::net_look::primary(ui, "Start"))
                .inner
                .on_disabled_hover_text("Stop recording with it first");
            if r.clicked() {
                net.mix_request = Some(true);
            }
        }
    });
}

/// Recording with the host: hear its mix, start at launch, the timeout and the takes folder.
fn recording_options(ui: &mut egui::Ui, net: &mut NetState, pick_folder: Option<&dyn Fn(&str) -> Option<String>>) {
    let t = Tokens::current();
    let cfg = net.hub.config();
    ui.add_space(6.0);
    egui::CollapsingHeader::new(egui::RichText::new("Recording options").color(t.text_dim)).id_salt("net-record-options").show(ui, |ui| {
        let mut hear = cfg.follow.hear_mix;
        if ui
            .checkbox(&mut hear, "Hear the host's mix")
            .on_hover_text("For a performer away from the host: the take lands where you played it against what you heard")
            .changed()
        {
            net.hub.update_config(|c| c.follow.hear_mix = hear);
        }
        let mut at_launch = cfg.follow.at_launch;
        if ui.checkbox(&mut at_launch, "Start recording with it when FreeDAWm starts").changed() {
            net.hub.update_config(|c| c.follow.at_launch = at_launch);
        }
        ui.horizontal(|ui| {
            ui.label("If the host goes away, end the take after");
            let mut min = cfg.contact_timeout_s / 60;
            if ui.add(egui::DragValue::new(&mut min).range(1..=1440).suffix(" min")).changed() {
                net.hub.update_config(|c| c.contact_timeout_s = min.max(1) * 60);
            }
        });
        if let Some(dir) = net.takes_dir() {
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new("Unsaved sessions are kept in").size(11.5).color(t.text_dim));
                ui.label(egui::RichText::new(dir.display().to_string()).size(11.5).monospace());
                if let Some(pick) = pick_folder
                    && ui.small_button("Change…").clicked()
                    && let Some(chosen) = pick(&dir.to_string_lossy())
                {
                    net.hub.update_config(|c| c.follow.takes_folder = chosen);
                }
            });
        }
    });
}

/// What this PC's screen says about the booth it follows (the toolbar badge), while following.
pub fn badge(app: &SoundApp) -> Option<FollowBadge> {
    let net = app.net.as_ref()?;
    if let Some(b) = &net.demo_badge {
        return Some(b.clone());
    }
    net.follower.as_ref().map(|f| net.follow.badge(f))
}

/// For renders (`ui_shot`'s `follower_demo`): this PC as a piano PC following "Booth", its Piano
/// track armed: `recording` (14 s into a take, notes played, the badge recording), `waiting`
/// (a take kept, waiting to be sent) or `sent`.
pub fn follower_demo(app: &mut SoundApp, state: &str) -> Result<(), String> {
    use soundcraft_playback::midi_in::{RecordedMidi, TYPING_SLOT};
    let badge = match state {
        "recording" => FollowBadge::Recording("Booth".into()),
        "waiting" => FollowBadge::Waiting(1),
        "sent" => FollowBadge::Sent("Booth".into()),
        _ => return Err(format!("unknown state `{state}` (recording, waiting or sent)")),
    };
    let Some(net) = app.net.as_mut() else { return Err("no network state yet (run a frame first)".into()) };
    net.demo_badge = Some(badge);
    let ex = |app: &mut SoundApp, id: &str, p: Value| app.engine.execute(id, &p).map_err(|e| e.to_string());
    let id = ex(app, "track.new", json!({"kind": "instrument", "name": "Piano"}))?["tracks"][0].as_u64().unwrap_or(0);
    ex(app, "track.input", json!({"tracks": [id], "input": "midi:all"}))?;
    ex(app, "mix.record_arm", json!({"tracks": [id], "value": true}))?;
    if state != "recording" {
        return Ok(());
    }
    let sr = i64::from(app.engine.session().sample_rate.hz());
    app.engine.session_mut().edit.selection = soundcraft_time::Range::point(2 * sr);
    app.start_recording(false);
    app.ui.status = "Recording with Booth".into();
    // Fourteen seconds into the take, notes played on the way.
    app.record_start = 2 * sr;
    app.sim = Some((14 * sr, None, None));
    app.engine.transport.position = 14 * sr;
    let port = TYPING_SLOT as u8;
    for i in 0..30i64 {
        let at = 2 * sr + sr * 2 / 5 + i * sr * 2 / 5;
        let pitch = [60u8, 64, 67, 72, 65, 69, 72, 77][i as usize % 8].saturating_sub(if i % 16 < 8 { 0 } else { 5 });
        app.midi_rec.push(RecordedMidi { at, port, msg: [0x90, pitch, 90], pass: 0 });
        if i < 29 {
            app.midi_rec.push(RecordedMidi { at: at + sr / 3, port, msg: [0x80, pitch, 0], pass: 0 });
        }
    }
    Ok(())
}

/// A QR code, `size` points square, dark modules on white with a quiet zone.
/// `net.remote_mix`: mix the booth picked in Setup › Network (`on: true`), stop (`on: false`),
/// toggle (no `on`); `hear` turns its mix in this PC's speakers on or off.
pub fn remote_mix_command(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let mixing = crate::remote_mix::active(app);
    let on = p.get("on").and_then(Value::as_bool).unwrap_or(!mixing);
    if let Some(h) = p.get("hear").and_then(Value::as_bool)
        && let Some(rm) = app.remote_mix.as_mut()
    {
        rm.hear = h;
    }
    if on && !mixing {
        let Some(net) = app.net.as_mut() else { return Err("the network is not ready yet".into()) };
        if net.follower.is_some() {
            return Err("Stop recording with the host before mixing it".into());
        }
        let cfg = net.hub.config();
        let over_internet = !cfg.follow.internet.is_empty();
        if cfg.follow.token.is_empty() || (cfg.follow.booth_addr.is_empty() && !over_internet) {
            return Err("Find the host and pair with it first (Setup › Network › Connect to a host)".into());
        }
        // Over the internet: through a local bridge to the booth, as recording with it is.
        let addr = if over_internet { net.bridge(&cfg.follow.internet)? } else { cfg.follow.booth_addr.clone() };
        let target = soundcraft_net::mixer::MixerTarget {
            host_name: cfg.follow.booth_name.clone(),
            addr,
            token: cfg.follow.token.clone(),
            my_name: net.hub.pc_name(),
        };
        let waker = net.hub.waker();
        crate::remote_mix::start(app, target, waker)?;
    } else if !on && mixing {
        crate::remote_mix::stop(app);
    }
    let rm = app.remote_mix.as_ref();
    Ok(json!({
        "on": rm.is_some(),
        "hear": rm.is_some_and(|r| r.hear),
        "connected": rm.is_some_and(|r| r.client.connected()),
        "status": rm.map(|r| r.client.status()),
    }))
}

/// While this PC mixes another: what it mixes and how the link is, Hear, and Stop.
fn mixing_banner(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(text) = crate::remote_mix::banner(app) else { return };
    let t = Tokens::current();
    let mut stop = false;
    let mut hear = app.remote_mix.as_ref().is_some_and(|r| r.hear);
    egui::Area::new(egui::Id::new("remote-mix-banner")).anchor(egui::Align2::CENTER_TOP, vec2(0.0, 30.0)).order(egui::Order::Foreground).show(
        ctx,
        |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(text).strong().color(t.text));
                    ui.checkbox(&mut hear, "Hear its mix");
                    if ui.button("Stop mixing").clicked() {
                        stop = true;
                    }
                });
            });
        },
    );
    if let Some(rm) = app.remote_mix.as_mut() {
        rm.hear = hear;
    }
    if stop {
        crate::remote_mix::stop(app);
    }
    ctx.request_repaint_after(Duration::from_millis(250));
}

fn draw_qr(ui: &mut egui::Ui, w: usize, m: &[bool], size: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, 4.0, Color32::WHITE);
    let quiet = 2.0;
    let cell = size / (w as f32 + 2.0 * quiet);
    for (i, dark) in m.iter().enumerate() {
        if *dark {
            let (x, y) = ((i % w) as f32 + quiet, (i / w) as f32 + quiet);
            p.rect_filled(egui::Rect::from_min_size(r.min + vec2(x * cell, y * cell), vec2(cell + 0.3, cell + 0.3)), 0.0, Color32::BLACK);
        }
    }
}

/// The remote recorders' audio inputs in an audio track's input menu (`Remote: <PC> › <input>`):
/// the track's take is recorded on that PC and lands here.
pub fn input_menu(app: &mut SoundApp, ui: &mut egui::Ui, id: TrackId, current: &Route) {
    let Some(track) = app.engine.session().track(id) else { return };
    if track.kind != soundcraft_model::TrackKind::Audio {
        return;
    }
    let inputs = app.net.as_ref().map(|n| n.hub.remote_inputs()).unwrap_or_default();
    if inputs.is_empty() {
        return;
    }
    ui.separator();
    ui.strong("Remote recorders");
    for name in inputs {
        let picked = matches!(current, Route::Hardware(h) if *h == name);
        if ui.add(egui::Button::selectable(picked, &name)).on_hover_text("Recorded on that PC; the take lands here when recording stops").clicked() {
            if let Err(e) = app.run("track.input", json!({"track": id.0, "input": name})) {
                app.ui.status = e;
            }
            ui.close();
        }
    }
}

/// A booth track on a remote recorder's audio input meters what that PC hears (its live level
/// over the network) and shows its clips.
pub fn with_remote_level(app: &SoundApp, track: &Track, mut md: crate::MeterDisplay) -> crate::MeterDisplay {
    let Route::Hardware(port) = &track.mixer.input else { return md };
    if !soundcraft_engine::io::is_remote_input(port) {
        return md;
    }
    let Some(net) = &app.net else { return md };
    if let Some(l) = net.hub.audio_level(port) {
        md.level = [l, l];
        md.peak = md.peak.max(l);
    }
    if net.hub.audio_sketches().iter().any(|s| s.port == *port && !s.clips.is_empty()) {
        md.clip = true;
    }
    md
}

/// On a booth track listening to a remote recorder (`Remote: <PC> › <port>`): the live sketch
/// while it records, a growing translucent clip with its notes, and an activity light; the exact
/// take replaces it when it lands.
pub fn draw_sketch(app: &SoundApp, painter: &egui::Painter, s: &Session, track: &Track, tl: egui::Rect, lane: egui::Rect) {
    let Some(net) = &app.net else { return };
    if let Route::Hardware(port) = &track.mixer.input
        && soundcraft_engine::io::is_remote_input(port)
    {
        draw_audio_sketch(app, net, painter, s, port, tl, lane);
        return;
    }
    let Route::Midi(m) = &track.mixer.input else { return };
    let Some(port) = m.device.as_deref().filter(|d| d.starts_with("Remote: ")) else { return };
    let sketches = net.hub.sketches();
    let Some(sk) = sketches.iter().find(|k| k.port == port) else { return };
    let rate = f64::from(s.sample_rate.hz().max(1));
    let at = |secs: f64| (secs * rate) as Samples;
    let start = app.take_plan.as_ref().map_or(app.record_start, |p| p.start);
    let end = if app.engine.transport.recording { app.position() } else { sk.notes.iter().map(|n| at(n.at)).max().unwrap_or(start) };
    let x0 = crate::edit_window::x_of(s, tl, start).max(lane.min.x);
    let x1 = crate::edit_window::x_of(s, tl, end).min(lane.max.x).max(x0 + 2.0);
    let r = egui::Rect::from_min_max(egui::pos2(x0, lane.min.y + 1.0), egui::pos2(x1, lane.max.y - 1.0));
    let t = Tokens::current();
    painter.rect(r, 2.0, t.rec.gamma_multiply(0.25), egui::Stroke::new(1.0, t.rec.gamma_multiply(0.8)), egui::StrokeKind::Inside);
    let lit = sk.last.elapsed() < Duration::from_millis(200);
    let dot = egui::pos2(r.min.x + 8.0, r.min.y + 8.0);
    painter.circle(dot, 4.0, if lit { Color32::from_rgb(90, 220, 110) } else { Color32::from_gray(60) }, egui::Stroke::new(1.0, Color32::BLACK));
    let label = format!("{} (live)", sk.pc);
    painter.with_clip_rect(r).text(egui::pos2(dot.x + 8.0, dot.y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(11.0), t.text);
    // The notes over the lane's height (the range played, at least an octave), each held until
    // its note-off.
    let body = egui::Rect::from_min_max(egui::pos2(r.min.x, r.min.y + 16.0), egui::pos2(r.max.x, r.max.y - 3.0));
    if body.height() < 6.0 {
        return;
    }
    let lo = sk.notes.iter().map(|n| n.pitch).min().unwrap_or(48);
    let hi = sk.notes.iter().map(|n| n.pitch).max().unwrap_or(72).max(lo.saturating_add(12));
    let span = f32::from(hi - lo).max(1.0);
    let y = |p: u8| body.max.y - f32::from(p.saturating_sub(lo)) / span * body.height();
    let mut held: Vec<(u8, f64)> = Vec::new();
    let p = painter.with_clip_rect(body);
    let col = Color32::from_rgb(255, 179, 181);
    for n in &sk.notes {
        if n.vel > 0 {
            held.push((n.pitch, n.at));
        } else if let Some(i) = held.iter().position(|h| h.0 == n.pitch) {
            let (pitch, from) = held.remove(i);
            let (a, b) = (crate::edit_window::x_of(s, tl, at(from)), crate::edit_window::x_of(s, tl, at(n.at)));
            p.line_segment([egui::pos2(a, y(pitch)), egui::pos2(b.max(a + 2.0), y(pitch))], egui::Stroke::new(2.0, col));
        }
    }
    for (pitch, from) in held {
        let a = crate::edit_window::x_of(s, tl, at(from));
        p.line_segment([egui::pos2(a, y(pitch)), egui::pos2(x1.max(a + 2.0), y(pitch))], egui::Stroke::new(2.0, col));
    }
}

/// A booth audio track on a remote input: while that PC records, a growing translucent clip with
/// its level sketch and an activity light; at every clip (live, and in landed takes) a red
/// marker.
fn draw_audio_sketch(app: &SoundApp, net: &NetState, painter: &egui::Painter, s: &Session, port: &str, tl: egui::Rect, lane: egui::Rect) {
    let rate = f64::from(s.sample_rate.hz().max(1));
    let at = |secs: f64| (secs * rate) as Samples;
    let x_at = |secs: f64| crate::edit_window::x_of(s, tl, at(secs));
    let t = Tokens::current();
    let red = Color32::from_rgb(255, 64, 64);
    let mark = |p: &egui::Painter, x: f32| {
        if x < lane.min.x || x > lane.max.x {
            return;
        }
        p.line_segment([egui::pos2(x, lane.min.y + 1.0), egui::pos2(x, lane.max.y - 1.0)], egui::Stroke::new(1.5, red));
        p.add(egui::Shape::convex_polygon(
            vec![egui::pos2(x - 4.0, lane.min.y + 1.0), egui::pos2(x + 4.0, lane.min.y + 1.0), egui::pos2(x, lane.min.y + 7.0)],
            red,
            egui::Stroke::NONE,
        ));
    };
    for (_, secs) in net.hub.clip_marks().iter().filter(|(p, _)| p == port) {
        mark(painter, x_at(*secs));
    }
    let sketches = net.hub.audio_sketches();
    let Some(sk) = sketches.iter().find(|k| k.port == port) else { return };
    let start = app.take_plan.as_ref().map_or(app.record_start, |p| p.start);
    let end = if app.engine.transport.recording { app.position() } else { sk.peaks.last().map_or(start, |p| at(p.0)) };
    let x0 = crate::edit_window::x_of(s, tl, start).max(lane.min.x);
    let x1 = crate::edit_window::x_of(s, tl, end).min(lane.max.x).max(x0 + 2.0);
    let r = egui::Rect::from_min_max(egui::pos2(x0, lane.min.y + 1.0), egui::pos2(x1, lane.max.y - 1.0));
    painter.rect(r, 2.0, t.rec.gamma_multiply(0.25), egui::Stroke::new(1.0, t.rec.gamma_multiply(0.8)), egui::StrokeKind::Inside);
    let body = egui::Rect::from_min_max(egui::pos2(r.min.x, r.min.y + 16.0), egui::pos2(r.max.x, r.max.y - 2.0));
    if body.height() >= 4.0 {
        // The level as a rough waveform: each peak mirrored about the middle, on a dB-ish scale.
        let p = painter.with_clip_rect(body);
        let mid = body.center().y;
        let half = body.height() * 0.5;
        let col = Color32::from_rgb(255, 179, 181);
        let mut last_x = f32::NEG_INFINITY;
        for (secs, peak) in &sk.peaks {
            let x = x_at(*secs);
            if x < body.min.x - 1.0 || x > body.max.x + 1.0 || x - last_x < 1.0 {
                continue;
            }
            last_x = x;
            let h = crate::widgets::meter_pos(20.0 * peak.max(1e-6).log10()).clamp(0.0, 1.0) * half;
            p.line_segment([egui::pos2(x, mid - h), egui::pos2(x, mid + h)], egui::Stroke::new(1.0, col));
        }
    }
    let lit = sk.last.elapsed() < Duration::from_millis(300);
    let dot = egui::pos2(r.min.x + 8.0, r.min.y + 8.0);
    painter.circle(dot, 4.0, if lit { Color32::from_rgb(90, 220, 110) } else { Color32::from_gray(60) }, egui::Stroke::new(1.0, Color32::BLACK));
    let n = sk.clips.len();
    let label = if n == 0 { format!("{} (live)", sk.pc) } else { format!("{} (live) · {n} clip{}", sk.pc, if n == 1 { "" } else { "s" }) };
    painter.with_clip_rect(r).text(
        egui::pos2(dot.x + 8.0, dot.y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(11.0),
        if n > 0 { red } else { t.text },
    );
    for c in &sk.clips {
        mark(painter, x_at(*c));
    }
}

/// For renders (`ui_shot`'s `net_demo`): a booth listening (port 48100, else any), with a
/// phone, the stage PC and a piano PC paired, the piano PC really connected in-process as a
/// remote recorder, and a booth track on its port showing a live sketch.
pub fn demo(app: &mut SoundApp) -> Result<(), String> {
    use soundcraft_net::config::PairedDevice;
    use soundcraft_net::proto::SketchNote;
    let Some(net) = app.net.take() else { return Err("no network state yet (run a frame first)".into()) };
    let piano = "demo-piano-token".to_string();
    net.hub.update_config(|c| {
        c.enabled = true;
        c.pc_name = "Booth".into();
        c.paired = vec![
            PairedDevice { name: "Phone".into(), kind: "browser".into(), token: "demo-phone".into(), follows: false },
            PairedDevice { name: "Stage PC".into(), kind: "browser".into(), token: "demo-stage".into(), follows: false },
            PairedDevice { name: "Piano PC".into(), kind: "pc".into(), token: piano.clone(), follows: true },
        ];
    });
    let port = net.hub.listen(soundcraft_net::config::DEFAULT_PORT).or_else(|_| net.hub.listen(0))?;
    let f = Follower::start(
        FollowTarget { booth_name: String::new(), addr: format!("127.0.0.1:{port}"), token: piano, my_name: "Piano PC".into() },
        None,
        None,
    );
    f.set_report(soundcraft_net::proto::FollowerReport {
        state: "recording".into(),
        notes: 1284,
        disk_free: Some(88_000_000_000),
        queued: 0,
        ports: vec!["Digital Piano".into()],
        ..soundcraft_net::proto::FollowerReport::default()
    });
    // Kept running for the render (it is this process's own "piano PC").
    std::mem::forget(f);
    let port_name = soundcraft_net::take::remote_port_name("Piano PC", "Digital Piano");
    let mut notes = Vec::new();
    for i in 0..48u32 {
        let at = 0.5 + f64::from(i) * 0.42;
        let pitch = [60u8, 64, 67, 72, 65, 69, 72, 77][i as usize % 8].saturating_sub(if i % 16 < 8 { 0 } else { 5 });
        notes.push(SketchNote { at, pitch, vel: 90 });
        notes.push(SketchNote { at: at + 0.35, pitch, vel: 0 });
    }
    net.hub.feed_sketch("Piano PC", "Digital Piano", &notes);
    app.net = Some(net);
    for (name, kind) in [("Vocal 1", "audio"), ("Vocal 2", "audio"), ("Acoustic", "audio"), ("Piano MIDI", "instrument")] {
        let id = app.engine.execute("track.new", &json!({"kind": kind, "name": name})).map_err(|e| e.to_string())?["tracks"][0].as_u64().unwrap_or(0);
        if kind == "instrument" {
            app.engine.execute("track.input", &json!({"tracks": [id], "input": format!("midi:{port_name}")})).map_err(|e| e.to_string())?;
        }
        app.engine.execute("mix.record_arm", &json!({"tracks": [id], "value": true})).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// For renders (`ui_shot`'s `{"net_demo": "internet"}`): Setup › Network on a booth letting a
/// performer join over the internet: an invite made, and a friend's PC joined in-process through
/// a real (local, relay-less) internet connection, following as a remote recorder.
pub fn demo_internet(app: &mut SoundApp) -> Result<(), String> {
    use soundcraft_net::internet::{Internet, Relays, key_hex, new_key};
    let Some(mut net) = app.net.take() else { return Err("no network state yet (run a frame first)".into()) };
    let key = new_key();
    net.hub.update_config(|c| {
        c.pc_name = "Main PC".into();
        c.internet_host = true;
        c.internet_key = key_hex(&key);
    });
    let host = Internet::start(key, Relays::LocalOnly)?;
    host.serve_to(net.hub.listen_internet()?);
    net.internet = Some(host);
    let code = net.make_invite(soundcraft_net::internet::ROLE_MIX)?;
    net.invite = Some((code, std::time::Instant::now()));
    // The friend: his own endpoint, joined with a second invite.
    let friend = Internet::start(new_key(), Relays::LocalOnly)?;
    let addr = net.internet.as_ref().map(|i| i.addr(Duration::from_secs(1))).ok_or("no endpoint")?;
    let bridge = friend.bridge_to(addr.clone())?.to_string();
    let secret = net.hub.new_invite_secret();
    let (token, _) = soundcraft_net::follower::pair_invite(&bridge, &secret, "Dave's PC")?;
    let f = Follower::start(FollowTarget { booth_name: "Main PC".into(), addr: bridge, token, my_name: "Dave's PC".into() }, None, None);
    f.set_report(soundcraft_net::proto::FollowerReport {
        state: "connected".into(),
        inputs: vec!["Vocal Mic".into()],
        check: Some("Level check: good, peak -9 dB".into()),
        ..soundcraft_net::proto::FollowerReport::default()
    });
    let end = std::time::Instant::now() + Duration::from_secs(5);
    while !f.connected() && std::time::Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    // Kept running for the render (this process's own "friend").
    std::mem::forget(f);
    std::mem::forget(friend);
    net.hub.update_config(|c| {
        c.enabled = true;
        c.paired.push(soundcraft_net::config::PairedDevice {
            name: "Alex's phone".into(),
            kind: "browser".into(),
            token: "demo-phone".into(),
            follows: false,
        });
    });
    net.start_listening();
    net.invite_role = soundcraft_net::internet::ROLE_MIX.into();
    app.net = Some(net);
    app.ui.show_network = true;
    Ok(())
}

/// For renders (`ui_shot`'s `{"net_demo": "audio"}`): a booth recording, a guitar PC connected
/// in-process as a remote audio recorder, the booth's Guitar track on its input (`Remote: Guitar
/// PC › In 1`) showing the live level sketch with two clips, and a Bass track with a landed take's
/// clip marker.
pub fn demo_audio(app: &mut SoundApp) -> Result<(), String> {
    use soundcraft_net::config::PairedDevice;
    let Some(net) = app.net.take() else { return Err("no network state yet (run a frame first)".into()) };
    let guitar = "demo-guitar-token".to_string();
    net.hub.update_config(|c| {
        c.enabled = true;
        c.pc_name = "Booth".into();
        c.paired = vec![
            PairedDevice { name: "Phone".into(), kind: "browser".into(), token: "demo-phone".into(), follows: false },
            PairedDevice { name: "Guitar PC".into(), kind: "pc".into(), token: guitar.clone(), follows: true },
        ];
    });
    let port = net.hub.listen(soundcraft_net::config::DEFAULT_PORT).or_else(|_| net.hub.listen(0))?;
    let f = Follower::start(
        FollowTarget { booth_name: String::new(), addr: format!("127.0.0.1:{port}"), token: guitar, my_name: "Guitar PC".into() },
        None,
        None,
    );
    f.set_report(soundcraft_net::proto::FollowerReport {
        state: "recording".into(),
        disk_free: Some(212_000_000_000),
        inputs: vec!["In 1".into()],
        clips: 2,
        ..soundcraft_net::proto::FollowerReport::default()
    });
    // Kept running for the render (it is this process's own "guitar PC").
    std::mem::forget(f);
    // Fourteen seconds of strumming from 2 s, louder in the chorus, clipping twice.
    let mut peaks = Vec::new();
    for i in 0..1200u32 {
        let t = 2.0 + f64::from(i) * 0.01;
        let beat = (t * 2.0).fract();
        let swell = if (7.0..11.0).contains(&t) { 0.85 } else { 0.45 };
        let p = (swell * (1.0 - beat * 0.7) * (0.8 + 0.2 * (t * 13.7).sin().abs())) as f32;
        peaks.push((t, p));
    }
    let clips = [7.5, 9.5];
    for c in clips {
        if let Some(p) = peaks.iter_mut().find(|p| (p.0 - c).abs() < 0.006) {
            p.1 = 1.0;
        }
    }
    let bass = soundcraft_net::take::remote_port_name("Bass PC", "Bass DI");
    net.hub.add_clip_marks(&bass, &[4.25]);
    app.net = Some(net);
    let ex = |app: &mut SoundApp, id: &str, p: Value| app.engine.execute(id, &p).map_err(|e| e.to_string());
    for (name, input) in [("Vocal 1", None), ("Guitar", Some("Remote: Guitar PC › In 1")), ("Bass", Some(bass.as_str()))] {
        let id = ex(app, "track.new", json!({"kind": "audio", "name": name}))?["tracks"][0].as_u64().unwrap_or(0);
        if let Some(i) = input {
            ex(app, "track.input", json!({"tracks": [id], "input": i}))?;
        }
        if name != "Bass" {
            ex(app, "mix.record_arm", json!({"tracks": [id], "value": true}))?;
        }
    }
    let sr = i64::from(app.engine.session().sample_rate.hz());
    app.engine.session_mut().edit.selection = soundcraft_time::Range::point(2 * sr);
    app.start_recording(false);
    app.record_start = 2 * sr;
    app.sim = Some((14 * sr, None, None));
    app.engine.transport.position = 14 * sr;
    // The booth's glue sees the Record (a new take clears old sketches), then the level arrives.
    let Some(mut net) = app.net.take() else { return Ok(()) };
    let _ = net.booth.step(&mut AppHost(app), &net.hub);
    net.hub.feed_audio_sketch("Guitar PC", "In 1", &peaks, &clips);
    app.ui.status = "Recording · Guitar PC recording with this host".into();
    app.net = Some(net);
    Ok(())
}

/// The performer's guard rails (spec section 4) on a remote recorder recording audio: one big
/// input meter, a clip lamp that stays lit until clicked, plain-language advice, and the
/// 10-second level check.
#[derive(Default)]
pub struct Guard {
    hold: soundcraft_net::guard::PeakHold,
    check: Option<soundcraft_net::guard::LevelCheck>,
    result: Option<(soundcraft_net::guard::Advice, String)>,
    clip_lamp: bool,
    clips_seen: usize,
    last: Option<std::time::Instant>,
    /// For renders: the input level shown (linear), and the check's progress.
    demo: Option<(f32, Option<f64>)>,
}

/// The input level of this PC's armed audio tracks (their meters, linear), `None` without one.
fn input_peak(app: &SoundApp) -> Option<f32> {
    let armed: Vec<TrackId> = app
        .engine
        .session()
        .tracks
        .iter()
        .filter(|t| t.mixer.record_arm && t.kind == soundcraft_model::TrackKind::Audio)
        .filter(|t| !matches!(&t.mixer.input, Route::Hardware(h) if soundcraft_engine::io::is_remote_input(h)))
        .map(|t| t.id)
        .collect();
    if armed.is_empty() {
        return None;
    }
    Some(armed.iter().filter_map(|id| app.meters.get(id)).map(|m| m.level[0].max(m.level[1])).fold(0f32, f32::max))
}

/// The performer's level window, while this PC follows a booth with an audio track armed.
pub fn performer_panel(app: &mut SoundApp, ctx: &egui::Context) {
    use soundcraft_net::guard::{Advice, LevelCheck, Zone, advice_live, db, zone};
    let Some(peak) = input_peak(app) else { return };
    let Some(mut net) = app.net.take() else { return };
    if net.follower.is_none() && net.guard.demo.is_none() {
        app.net = Some(net);
        return;
    }
    let (peak, demo_progress) = match net.guard.demo {
        Some((l, p)) => (l, p),
        None => (peak, None),
    };
    let now = std::time::Instant::now();
    let dt = net.guard.last.map_or(0.0, |t| now.duration_since(t).as_secs_f64());
    net.guard.last = Some(now);
    net.guard.hold.feed(peak, dt, 3.0);
    if peak >= soundcraft_net::audio::CLIP_LEVEL || net.follow.clips.len() > net.guard.clips_seen {
        net.guard.clip_lamp = true;
    }
    net.guard.clips_seen = net.follow.clips.len();
    if let Some(c) = &mut net.guard.check {
        c.feed(peak, dt);
        if let Some(a) = c.result() {
            net.guard.result = Some((a, c.summary().unwrap_or_default()));
            net.follow.check = c.summary();
            net.guard.check = None;
        }
    }
    let t = Tokens::current();
    let green = Color32::from_rgb(63, 185, 80);
    let amber = Color32::from_rgb(240, 198, 116);
    let red = Color32::from_rgb(255, 64, 64);
    let booth = net.follower.as_ref().map_or_else(|| "Booth".to_string(), soundcraft_net::glue::booth_name);
    let mut start_check = false;
    let mut clear_lamp = false;
    egui::Window::new("Your level")
        .id(egui::Id::new("net-performer-level"))
        .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-16.0, -40.0))
        .collapsible(true)
        .resizable(false)
        .default_width(380.0)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new(format!("What {booth} records from this PC")).small().color(t.text_dim));
            ui.horizontal(|ui| {
                // The meter: -60..0 dBFS, the target range marked, the bar in its zone's colour.
                let (whole, _) = ui.allocate_exact_size(vec2(300.0, 46.0), Sense::hover());
                let r = egui::Rect::from_min_max(whole.min, egui::pos2(whole.max.x, whole.max.y - 12.0));
                let p = ui.painter();
                p.rect_filled(r, 3.0, Color32::from_gray(24));
                let x = |d: f32| r.min.x + crate::widgets::meter_pos(d).clamp(0.0, 1.0) * r.width();
                let band = egui::Rect::from_min_max(egui::pos2(x(-12.0), r.min.y), egui::pos2(x(-6.0), r.max.y));
                p.rect_filled(band, 0.0, green.gamma_multiply(0.18));
                let d = db(peak);
                let col = match zone(d) {
                    Zone::Green => green,
                    Zone::Amber => amber,
                    Zone::Red => red,
                };
                p.rect_filled(egui::Rect::from_min_max(r.min + vec2(0.0, 5.0), egui::pos2(x(d), r.max.y - 5.0)), 2.0, col);
                let held = net.guard.hold.max_db();
                if held > -100.0 {
                    p.line_segment([egui::pos2(x(held), r.min.y + 2.0), egui::pos2(x(held), r.max.y - 2.0)], egui::Stroke::new(2.0, Color32::WHITE));
                }
                for (mark, label) in [(-48.0, "-48"), (-24.0, "-24"), (-12.0, "-12"), (-6.0, "-6"), (0.0, "0 dB")] {
                    let mx = x(mark);
                    p.line_segment([egui::pos2(mx, r.max.y), egui::pos2(mx, r.max.y + 3.0)], egui::Stroke::new(1.0, t.text_dim));
                    let align = if mark == 0.0 { egui::Align2::RIGHT_BOTTOM } else { egui::Align2::CENTER_BOTTOM };
                    p.text(egui::pos2(mx, whole.max.y), align, label, egui::FontId::proportional(9.0), t.text_dim);
                }
                // The clip lamp: lit until clicked.
                let (lr, resp) = ui.allocate_exact_size(vec2(52.0, 34.0), Sense::click());
                let lit = net.guard.clip_lamp;
                ui.painter().rect(
                    lr,
                    4.0,
                    if lit { red } else { Color32::from_gray(40) },
                    egui::Stroke::new(1.0, if lit { Color32::WHITE } else { Color32::from_gray(70) }),
                    egui::StrokeKind::Inside,
                );
                ui.painter().text(
                    lr.center(),
                    egui::Align2::CENTER_CENTER,
                    "CLIP",
                    egui::FontId::proportional(13.0),
                    if lit { Color32::WHITE } else { t.text_dim },
                );
                if resp.on_hover_text("Lights when the input clipped; click to clear").clicked() {
                    clear_lamp = true;
                }
            });
            let advice = advice_live(net.guard.hold.max_db(), net.guard.clip_lamp);
            let col = match advice {
                Advice::Good => green,
                Advice::Clipped | Advice::TooLoud => red,
                _ => amber,
            };
            ui.label(egui::RichText::new(advice.text()).size(16.0).strong().color(col));
            // The booth's mix in his ears, and how it is getting here.
            let follow = net.hub.config().follow;
            if follow.hear_mix || net.guard.demo.is_some() {
                ui.horizontal(|ui| {
                    ui.label("Mix volume");
                    let mut db_gain = 20.0 * follow.mix_gain.max(1e-4).log10();
                    if ui.add(egui::Slider::new(&mut db_gain, -40.0..=6.0).suffix(" dB").step_by(0.5)).changed() {
                        let g = 10f32.powf(db_gain / 20.0);
                        net.hub.update_config(|c| c.follow.mix_gain = if db_gain <= -39.9 { 0.0 } else { g });
                    }
                });
                let health = net.follower.as_ref().and_then(soundcraft_net::follower::Follower::stream_health);
                let line = match (&health, &net.guard.demo) {
                    (Some(h), _) if h.packets > 0 => {
                        let loss = if h.packets > 0 { h.lost as f64 * 100.0 / h.packets as f64 } else { 0.0 };
                        format!("Mix: {:.0} ms behind the host · {loss:.1} % lost", h.buffer_ms)
                    }
                    (_, Some(_)) => "Mix: 150 ms behind the host · 0.0 % lost".to_string(),
                    _ => "Mix: waiting for the host to play".to_string(),
                };
                ui.label(egui::RichText::new(line).small().color(t.text_dim));
                ui.label(
                    egui::RichText::new("You hear yourself straight from this PC (no delay); the host hears you on the call.")
                        .small()
                        .color(t.text_dim),
                );
            }
            ui.add_space(4.0);
            match net.guard.check.as_ref().map(LevelCheck::progress).or(demo_progress) {
                Some(progress) => {
                    ui.label("Level check: play or sing your loudest part…");
                    ui.add(egui::ProgressBar::new(progress as f32).desired_width(352.0).text(format!("{:.0} s left", (1.0 - progress) * 10.0)));
                }
                _ => {
                    ui.horizontal(|ui| {
                        if ui
                            .button("Level check (10 s)")
                            .on_hover_text("Play or sing your loudest part: FreeDAWm says turn up, turn down or good")
                            .clicked()
                        {
                            start_check = true;
                        }
                        if let Some((a, summary)) = &net.guard.result {
                            let c = if *a == Advice::Good { green } else { amber };
                            ui.label(egui::RichText::new(summary).color(c));
                        }
                    });
                    if let Some((a, _)) = &net.guard.result
                        && a.warning()
                    {
                        ui.label(
                            egui::RichText::new(match a {
                                Advice::TooQuiet => "Turn the knob up a little, then check again.",
                                Advice::Nothing => "Nothing came in: check the cable and the input, then check again.",
                                _ => "Turn the knob down a little, then check again.",
                            })
                            .color(t.text_dim),
                        );
                    }
                }
            }
        });
    if clear_lamp {
        net.guard.clip_lamp = false;
    }
    if start_check {
        net.guard.check = Some(LevelCheck::new());
        net.guard.result = None;
    }
    ctx.request_repaint_after(Duration::from_millis(50));
    app.net = Some(net);
}

/// For renders (`ui_shot`'s `{"performer_demo": "loud"}`, `"check"` or `"good"`): this PC as a
/// guitar PC following "Booth", its Guitar track armed, the level window showing a clip and
/// "too loud", a level check half done, or a good check.
pub fn performer_demo(app: &mut SoundApp, state: &str) -> Result<(), String> {
    use soundcraft_net::guard::{Advice, LevelCheck};
    let (level, progress, lamp, result) = match state {
        "loud" => (0.93f32, None, true, None),
        "check" => (0.4, Some(0.45), false, None),
        "good" => {
            let mut c = LevelCheck::new();
            c.feed(0.38, 1.0);
            for _ in 0..10 {
                c.feed(0.2, 1.0);
            }
            (0.3, None, false, Some((Advice::Good, c.summary().unwrap_or_default())))
        }
        _ => return Err(format!("unknown state `{state}` (loud, check or good)")),
    };
    let Some(net) = app.net.as_mut() else { return Err("no network state yet (run a frame first)".into()) };
    net.guard.demo = Some((level, progress));
    net.guard.clip_lamp = lamp;
    net.guard.result = result;
    net.demo_badge = Some(FollowBadge::Following("Booth".into()));
    let id =
        app.engine.execute("track.new", &json!({"kind": "audio", "name": "Guitar"})).map_err(|e| e.to_string())?["tracks"][0].as_u64().unwrap_or(0);
    app.engine.execute("mix.record_arm", &json!({"tracks": [id], "value": true})).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_net::config::PairedDevice;

    /// A booth: an engine whose transport the test sets.
    struct Booth {
        e: Engine,
        view: HostView,
    }

    impl RigHost for Booth {
        fn engine(&self) -> &Engine {
            &self.e
        }
        fn engine_mut(&mut self) -> &mut Engine {
            &mut self.e
        }
        fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
            self.e.execute(id, &params).map_err(|e| e.to_string())
        }
        fn view(&self) -> HostView {
            self.view
        }
    }

    /// A piano PC following a booth in this process: when the booth records, its own screen shows
    /// a local recording (record state, `REC with Booth`, the badge, the MIDI take growing on its
    /// armed track); after Stop the take lands on the booth and the badge says so.
    #[test]
    fn a_followers_take_shows_as_a_local_recording_and_then_as_sent() {
        use egui_kittest::kittest::Queryable;
        let hub = Hub::new(NetConfig { pc_name: "Booth".into(), ..NetConfig::default() }, None);
        hub.update_config(|c| {
            c.paired = vec![PairedDevice { name: "Piano PC".into(), kind: "pc".into(), token: "piano".into(), follows: true }];
        });
        let port = hub.listen(0).unwrap();
        let mut booth = Booth { e: Engine::default(), view: HostView::default() };
        let mut bglue = BoothGlue::default();
        let mut app = SoundApp::new(Engine::default(), None, crate::Services::default());
        let piano = app.engine.execute("track.new", &json!({"kind": "instrument", "name": "Piano"})).unwrap()["tracks"][0].as_u64().unwrap();
        app.engine.execute("track.input", &json!({"tracks": [piano], "input": "midi:all"})).unwrap();
        app.engine.execute("mix.record_arm", &json!({"tracks": [piano], "value": true})).unwrap();
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1600.0, 900.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(&ui.ctx().clone());
                app.ui(ui);
            },
            app,
        );
        h.step();
        let target =
            FollowTarget { booth_name: "Booth".into(), addr: format!("127.0.0.1:{port}"), token: "piano".into(), my_name: "Piano PC".into() };
        let f = Follower::start(target, None, None);
        h.state_mut().net.as_mut().unwrap().follower = Some(f.clone());
        let mut frames = |h: &mut egui_kittest::Harness<'_, SoundApp>, booth: &mut Booth, what: &str, done: &dyn Fn(&SoundApp) -> bool| {
            let end = std::time::Instant::now() + Duration::from_secs(15);
            loop {
                let _ = bglue.step(booth, &hub);
                h.step();
                if done(h.state()) {
                    return;
                }
                assert!(std::time::Instant::now() < end, "timed out: {what} (status: {})", h.state().ui.status);
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        frames(&mut h, &mut booth, "connected", &|a| a.net.as_ref().and_then(|n| n.follower.as_ref()).is_some_and(Follower::connected));
        assert_eq!(badge(h.state()).map(|b| b.text()), Some("Following Booth".into()));
        // The booth records.
        booth.view = HostView { playing: true, recording: true, ..HostView::default() };
        frames(&mut h, &mut booth, "the piano PC records", &|a| a.engine.transport.recording);
        assert_eq!(badge(h.state()).map(|b| b.text()), Some("Recording with Booth".into()));
        assert_eq!(h.state().ui.status, "Recording with Booth");
        let typing = soundcraft_playback::midi_in::TYPING_SLOT as u8;
        let at = h.state().position();
        h.state_mut().midi_rec.push(soundcraft_playback::midi_in::RecordedMidi { at, port: typing, msg: [0x90, 60, 100], pass: 0 });
        for _ in 0..10 {
            h.step();
        }
        let track = h.state().engine.session().track(TrackId(piano)).unwrap().clone();
        let first = crate::live_midi::live_take(h.state(), &track).expect("a live MIDI take");
        assert_eq!(first.notes.len(), 1);
        assert!(crate::toolbar::record_line(h.state(), h.state().position()).starts_with("● REC with Booth"));
        assert!(h.query_by_label("Recording with Booth").is_some(), "the toolbar badge");
        std::thread::sleep(Duration::from_millis(100));
        for _ in 0..10 {
            h.step();
        }
        let later = crate::live_midi::live_take(h.state(), &track).unwrap();
        assert!(later.end > first.end, "the take grows: {} then {}", first.end, later.end);
        let at = h.state().position();
        h.state_mut().midi_rec.push(soundcraft_playback::midi_in::RecordedMidi { at, port: typing, msg: [0x80, 60, 0], pass: 0 });
        // The booth stops (ten seconds in): the piano PC stops, keeps the take as a clip, and sends it.
        booth.view.position = 480_000;
        frames(&mut h, &mut booth, "one frame", &|_| true);
        booth.view = HostView::default();
        frames(&mut h, &mut booth, "the take lands", &|a| {
            !a.engine.transport.recording && badge(a).is_some_and(|b| b == FollowBadge::Sent("Booth".into()))
        });
        assert_eq!(h.state().engine.session().track(TrackId(piano)).unwrap().clips().len(), 1, "the local copy");
        assert_eq!(h.state().ui.status, "Take sent to Booth");
        assert!(booth.e.session().tracks.iter().any(|t| !t.clips().is_empty()), "the host has the take");
        f.stop();
        hub.stop();
    }

    /// This app mixes a booth in the same process: the booth's session is on screen in place of
    /// its own, a fader change lands at the booth, the booth's transport and the banner show, and
    /// stopping brings its own session back as it was.
    #[test]
    fn this_pc_mixes_a_booth_and_gets_its_own_session_back() {
        use egui_kittest::kittest::Queryable;
        let hub = Hub::new(NetConfig { pc_name: "Booth".into(), ..NetConfig::default() }, None);
        hub.update_config(|c| {
            c.paired = vec![PairedDevice { name: "Mixer PC".into(), kind: "pc".into(), token: "mixer".into(), follows: false }];
        });
        let port = hub.listen(0).unwrap();
        let mut booth = Booth { e: Engine::default(), view: HostView::default() };
        booth.e.execute("track.new", &json!({"count": 1, "format": "Mono", "name": "Lead Vox"})).unwrap();
        let vox = booth.e.session().tracks.iter().find(|t| t.name == "Lead Vox").unwrap().id;
        let mut bglue = BoothGlue::default();
        let mut app = SoundApp::new(Engine::default(), None, crate::Services::default());
        app.engine.execute("track.new", &json!({"count": 1, "format": "Mono", "name": "Mine"})).unwrap();
        let mine_steps = app.engine.undo_history();
        let mut h = egui_kittest::Harness::builder().with_size(vec2(1600.0, 900.0)).build_ui_state(
            |ui, app: &mut SoundApp| {
                app.logic(&ui.ctx().clone());
                app.ui(ui);
            },
            app,
        );
        h.step();
        let target = soundcraft_net::mixer::MixerTarget {
            host_name: "Booth".into(),
            addr: format!("127.0.0.1:{port}"),
            token: "mixer".into(),
            my_name: "Mixer PC".into(),
        };
        crate::remote_mix::start(h.state_mut(), target, None).unwrap();
        let frames = |h: &mut egui_kittest::Harness<'_, SoundApp>,
                      bglue: &mut BoothGlue,
                      booth: &mut Booth,
                      what: &str,
                      done: &dyn Fn(&SoundApp, &Booth) -> bool| {
            let end = std::time::Instant::now() + Duration::from_secs(15);
            loop {
                for r in bglue.step(booth, &hub) {
                    if let HubRequest::Control { params, reply, .. } = r {
                        let id = params["command"].as_str().unwrap_or("").to_string();
                        let p = params.get("params").cloned().unwrap_or(json!({}));
                        let got = match params.get("merge").and_then(Value::as_str) {
                            Some(k) => booth.e.execute_merged(&id, &p, k),
                            None => booth.e.execute(&id, &p),
                        };
                        let _ = reply.send(got.map_or_else(|e| json!({"ok": false, "error": e.to_string()}), |v| json!({"ok": true, "result": v})));
                    }
                }
                h.step();
                if done(h.state(), booth) {
                    return;
                }
                assert!(std::time::Instant::now() < end, "timed out: {what} (status: {})", h.state().ui.status);
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        frames(&mut h, &mut bglue, &mut booth, "the host's session arrives", &|a, _| a.engine.session().tracks.iter().any(|t| t.name == "Lead Vox"));
        assert!(!h.state().engine.session().tracks.iter().any(|t| t.name == "Mine"));
        assert_eq!(h.state().ui.window, crate::MainWindow::Mix);
        assert!(h.query_by_label_contains("Mixing Booth remotely: changes go to Booth").is_some(), "the banner");
        // A fader change here lands at the booth.
        h.state_mut().run("mix.volume", json!({"track": vox.0, "db": -4.5})).unwrap();
        assert_eq!(h.state().engine.session().track(vox).unwrap().mixer.volume_db, -4.5, "at once here");
        frames(&mut h, &mut bglue, &mut booth, "the host's fader moves", &|_, b| b.e.session().track(vox).unwrap().mixer.volume_db == -4.5);
        // The booth's transport shows here; nothing renders here.
        booth.view = HostView { playing: true, position: 96_000, ..HostView::default() };
        frames(&mut h, &mut bglue, &mut booth, "the host plays", &|a, _| a.is_playing() && a.engine.transport.position == 96_000);
        // Editing is refused here, and saving never touches this PC's own session.
        assert!(h.state_mut().run("track.new", json!({"count": 1})).is_err());
        assert!(!h.state().engine.is_dirty());
        // Stop mixing: this PC's own session is back, history and all.
        crate::remote_mix::stop(h.state_mut());
        h.step();
        assert!(h.state().engine.session().tracks.iter().any(|t| t.name == "Mine"));
        assert_eq!(h.state().engine.undo_history(), mine_steps);
        assert!(!h.state().is_playing());
        assert!(h.query_by_label_contains("Mixing Booth remotely").is_none());
        hub.stop();
    }

    #[test]
    fn today_is_a_date() {
        let d = super::today();
        assert_eq!(d.len(), 10);
        assert!(d.starts_with("20"));
        assert_eq!(&d[4..5], "-");
    }
}
