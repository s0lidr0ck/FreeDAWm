//! The listening side: one TCP port on the local network serving
//!
//! - the service remote page and its HTTP API (`GET /`, `GET /api/events` — the status feed as
//!   server-sent events —, `POST /api/pair/*`, `POST /api/action`), and
//! - the JSON-lines control channel for paired PCs (the same protocol as the `--control`
//!   channel, plus `pair.*`, `hello`, `sync.ping`, `follower.report`, `feed` and
//!   `take.transfer` for remote recorders).
//!
//! The first bytes of a connection tell which (`GET`/`POST` or `{`). Everything here runs on
//! network threads; what needs the session goes to the UI thread as a [`HubRequest`] (drained
//! each frame), and the status the UI publishes ([`Hub::publish`]) is pushed from here. Nothing
//! touches the audio thread: the booth's take clock is read through a [`TakeClock`] (atomics).

use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{Duration, Instant};

use crate::config::{NetConfig, PairedDevice};
use crate::pairing::{Confirm, Pairing, PendingPair};
use crate::proto::{BoothTransport, FollowerReport, RemoteAction, RemoteStatus, RigStatus, SketchNote, TransportState};
use crate::take::{BoothAnchor, TakeEnvelope, remote_port_name};

/// A PC's take clock (see [`crate::sync`]): seconds rolled since its transport started, as
/// audible now; `None` while stopped. Read on network threads: lock-free behind it.
pub trait TakeClock: Send + Sync {
    fn now(&self) -> Option<f64>;
}

/// What the network asks of the UI thread.
pub enum HubRequest {
    /// A paired PC's control-protocol request (any method), answered on `reply` with the same
    /// `{ok, result | error}` value the `--control` channel sends.
    Control { method: String, params: Value, reply: Sender<Value> },
    /// A service remote button, from the paired device `device`.
    Action { action: RemoteAction, device: String, reply: Sender<Result<Value, String>> },
    /// A remote recorder's finished take, to land; answered once it is in the session. An audio
    /// take comes `prepared`: its file already made in the booth project, on the booth's clock.
    Take { take: Box<TakeEnvelope>, prepared: Option<PreparedAudio>, reply: Sender<Result<Value, String>> },
}

/// An audio take made ready to land (on a network thread, so the UI never waits for it): its
/// file in the booth project's `Audio Files` at the session rate and bit depth, the booth
/// take-clock sample of its first frame ([`crate::audio::align`]), and its clips on the booth
/// timeline (seconds).
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedAudio {
    pub wav: PathBuf,
    pub first: i64,
    pub converted: bool,
    pub drift: f64,
    pub clips: Vec<f64>,
}

/// Where the booth keeps remote audio and the format it lands in (published by the UI thread).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectInfo {
    /// The session's project folder (`None` while it was never saved).
    pub folder: Option<PathBuf>,
    pub rate: u32,
    /// 16, 24 or 32 (float).
    pub bits: u16,
}

/// The live level of one remote audio input (booth timeline seconds): a peak every ~20 ms while it
/// records, and where it clipped.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioSketch {
    /// `Remote: <PC> › <input>`.
    pub port: String,
    pub pc: String,
    pub peaks: Vec<(f64, f32)>,
    pub clips: Vec<f64>,
    /// The latest peak (the input meter), and when it came.
    pub level: f32,
    pub last: Instant,
}

/// A transfer file arriving.
#[derive(Debug, Clone)]
struct Upload {
    id: String,
    pc: String,
    bytes: u64,
    have: u64,
    hash: String,
    part: PathBuf,
    done: PathBuf,
}

/// Connections served at once (browsers, PCs); further ones are closed.
pub const MAX_CONNECTIONS: usize = 32;
/// Longest HTTP header block and body.
const MAX_HEAD: usize = 16 << 10;
const MAX_BODY: usize = 64 << 10;
/// Longest JSON line from a PC (a take of a whole service, as JSON).
const MAX_LINE: usize = 64 << 20;
/// Status pushes at most this often (~15 Hz).
const STATUS_EVERY: Duration = Duration::from_millis(66);
/// A PC link without a line for this long is dead (they report every second).
const LINK_TIMEOUT: Duration = Duration::from_secs(15);

const PAGE: &str = include_str!("../web/index.html");
const SCRIPT: &str = include_str!("../web/remote.js");
const STYLE: &str = include_str!("../web/remote.css");

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A fresh random number from the OS (for codes and tokens).
pub fn random_u64() -> u64 {
    let mut b = [0u8; 8];
    if getrandom::fill(&mut b).is_err() {
        // No OS randomness: the std hasher's per-process random keys and the time.
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        return h.finish();
    }
    u64::from_le_bytes(b)
}

/// A pairing token: 128 random bits as hex.
pub fn new_token() -> String {
    format!("{:016x}{:016x}", random_u64(), random_u64())
}

/// This computer's name (the default PC name).
pub fn computer_name() -> String {
    let n = gethostname::gethostname().to_string_lossy().trim().to_string();
    if n.is_empty() { "FreeDAWm".into() } else { n }
}

/// A remote recorder connected now (or before).
#[derive(Debug, Clone)]
struct Link {
    id: u64,
    name: String,
    report: FollowerReport,
    connected: bool,
    last_seen: Instant,
    /// A take from it landed since its last Record.
    landed: bool,
    /// It hears the booth's mix (the stream).
    streaming: bool,
}

/// The live sketch of one remote port (booth timeline seconds).
#[derive(Debug, Clone, PartialEq)]
pub struct Sketch {
    /// `Remote: <PC> › <port>`.
    pub port: String,
    pub pc: String,
    pub notes: Vec<SketchNote>,
    /// When the last message came (the activity light).
    pub last: Instant,
}

/// The most sketch notes kept per port.
const MAX_SKETCH: usize = 200_000;

struct Listening {
    port: u16,
    stop: Arc<AtomicBool>,
}

struct Inner {
    cfg: Mutex<NetConfig>,
    cfg_path: Option<PathBuf>,
    pairing: Mutex<Pairing>,
    started: Instant,
    /// The published status, its JSON and a change counter; `changed` wakes the pushers.
    status: Mutex<(u64, RigStatus, Arc<String>)>,
    transport: Mutex<(u64, BoothTransport)>,
    /// Wakes the status pushers (with `status`) and the transport pushers (with `transport`).
    changed: Condvar,
    transport_changed: Condvar,
    tx: Mutex<Sender<HubRequest>>,
    rx: Mutex<Receiver<HubRequest>>,
    clock: RwLock<Option<Arc<dyn TakeClock>>>,
    links: Mutex<Vec<Link>>,
    clients: Mutex<Vec<(u64, String)>>,
    sketches: Mutex<Vec<Sketch>>,
    audio: Mutex<Vec<AudioSketch>>,
    /// Clips of landed remote audio takes: (port, booth timeline seconds).
    clip_marks: Mutex<Vec<(String, f64)>>,
    uploads: Mutex<Vec<Upload>>,
    project: Mutex<ProjectInfo>,
    /// The remote mix stream: the player's tap and the session rate, the links hearing it, and
    /// whether its thread runs.
    tap: Mutex<Option<(Arc<soundcraft_playback::stream::MixTap>, u32)>>,
    stream_subs: Mutex<Vec<(u64, Arc<Mutex<TcpStream>>)>>,
    streaming: AtomicBool,
    listening: Mutex<Option<Listening>>,
    /// The loopback listener the internet bridge connects to (only pairing by invite there).
    internet: Mutex<Option<Listening>>,
    /// Invites made (their one-time secret and when they expire, hub seconds).
    invites: Mutex<Vec<(String, f64)>>,
    open: AtomicUsize,
    next_id: AtomicU64,
    warning: Mutex<Option<String>>,
    /// The booth's playhead now (seconds), for a follower's greeting.
    position: Mutex<f64>,
    /// Wakes the UI thread when a request comes in.
    waker: Mutex<Option<Waker>>,
    /// Remote mixing: the session in pieces and the meters for the mixers, and the mixers
    /// connected (their names).
    mix: Mutex<MixStore>,
    mix_changed: Condvar,
    mixers: Mutex<Vec<(u64, String)>>,
}

/// The session in pieces as last published for the mixers, each with the change counter it was
/// last changed at, and the latest meter frame (its JSON line).
#[derive(Default)]
struct MixStore {
    seq: u64,
    rest: (u64, Arc<String>),
    order: (u64, Arc<String>),
    tracks: Vec<(u64, u64, Arc<String>)>,
    meters: (u64, Arc<String>),
}

/// Wakes the app's UI thread (egui's `request_repaint`).
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// The network hub of one PC (cheap to clone; all clones are the same hub).
#[derive(Clone)]
pub struct Hub {
    inner: Arc<Inner>,
}

impl Hub {
    /// A hub with `cfg`, saved to `cfg_path` when it changes (pairings, landed takes).
    pub fn new(cfg: NetConfig, cfg_path: Option<PathBuf>) -> Hub {
        let (tx, rx) = channel();
        Hub {
            inner: Arc::new(Inner {
                cfg: Mutex::new(cfg),
                cfg_path,
                pairing: Mutex::new(Pairing::default()),
                started: Instant::now(),
                status: Mutex::new((0, RigStatus::default(), Arc::new("{}".into()))),
                transport: Mutex::new((0, BoothTransport::default())),
                changed: Condvar::new(),
                transport_changed: Condvar::new(),
                tx: Mutex::new(tx),
                rx: Mutex::new(rx),
                clock: RwLock::new(None),
                links: Mutex::new(Vec::new()),
                clients: Mutex::new(Vec::new()),
                sketches: Mutex::new(Vec::new()),
                audio: Mutex::new(Vec::new()),
                clip_marks: Mutex::new(Vec::new()),
                uploads: Mutex::new(Vec::new()),
                project: Mutex::new(ProjectInfo::default()),
                tap: Mutex::new(None),
                stream_subs: Mutex::new(Vec::new()),
                streaming: AtomicBool::new(false),
                listening: Mutex::new(None),
                internet: Mutex::new(None),
                invites: Mutex::new(Vec::new()),
                open: AtomicUsize::new(0),
                next_id: AtomicU64::new(1),
                warning: Mutex::new(None),
                position: Mutex::new(0.0),
                waker: Mutex::new(None),
                mix: Mutex::new(MixStore::default()),
                mix_changed: Condvar::new(),
                mixers: Mutex::new(Vec::new()),
            }),
        }
    }

    fn now_s(&self) -> f64 {
        self.inner.started.elapsed().as_secs_f64()
    }

    pub fn config(&self) -> NetConfig {
        lock(&self.inner.cfg).clone()
    }

    /// Changes the settings and saves them.
    pub fn update_config(&self, f: impl FnOnce(&mut NetConfig)) {
        let mut c = lock(&self.inner.cfg);
        f(&mut c);
        if let Some(p) = &self.inner.cfg_path
            && let Err(e) = c.save(p)
        {
            log::warn!("network settings: {e}");
        }
    }

    /// This PC's name on the network.
    pub fn pc_name(&self) -> String {
        let n = lock(&self.inner.cfg).pc_name.trim().to_string();
        if n.is_empty() { computer_name() } else { n }
    }

    pub fn set_clock(&self, clock: Option<Arc<dyn TakeClock>>) {
        *self.inner.clock.write().unwrap_or_else(PoisonError::into_inner) = clock;
    }

    fn clock_now(&self) -> Option<f64> {
        self.inner.clock.read().unwrap_or_else(PoisonError::into_inner).as_ref().and_then(|c| c.now())
    }

    /// The requests waiting for the UI thread (call once a frame).
    pub fn drain(&self) -> Vec<HubRequest> {
        let rx = lock(&self.inner.rx);
        let mut v = Vec::new();
        while let Ok(r) = rx.try_recv() {
            v.push(r);
            if v.len() >= 64 {
                break;
            }
        }
        v
    }

    fn send(&self, r: HubRequest) -> bool {
        let sent = lock(&self.inner.tx).send(r).is_ok();
        if let Some(w) = lock(&self.inner.waker).as_ref() {
            w();
        }
        sent
    }

    /// What wakes the UI thread (for other links of this PC).
    pub fn waker(&self) -> Option<Waker> {
        lock(&self.inner.waker).clone()
    }

    fn wake(&self) {
        if let Some(w) = lock(&self.inner.waker).clone() {
            w();
        }
    }

    pub fn set_waker(&self, w: Option<Waker>) {
        *lock(&self.inner.waker) = w;
    }

    /// Pairing requests waiting for their code (shown on this PC's screen).
    pub fn pending_pairings(&self) -> Vec<PendingPair> {
        lock(&self.inner.pairing).pending(self.now_s())
    }

    pub fn deny_pairing(&self, id: &str) {
        lock(&self.inner.pairing).deny(id);
    }

    pub fn forget(&self, token: &str) {
        self.update_config(|c| c.paired.retain(|d| d.token != token));
    }

    fn pair_request(&self, name: &str, kind: &str) -> Result<Value, String> {
        let now = self.now_s();
        let r = lock(&self.inner.pairing).request(name, kind, now, random_u64).ok_or("too many pairing requests; try again in a few minutes")?;
        log::info!("pairing requested by {} ({}): code shown on screen", r.name, r.kind);
        Ok(json!({"request": r.id, "pc": self.pc_name()}))
    }

    fn pair_confirm(&self, id: &str, code: &str) -> Result<Value, (String, u8)> {
        let now = self.now_s();
        let got = lock(&self.inner.pairing).confirm(id, code, now);
        match got {
            Confirm::Paired { name, kind } => {
                let token = new_token();
                let dev = PairedDevice { name: name.clone(), kind, token: token.clone(), follows: false };
                self.update_config(|c| c.paired.push(dev));
                Ok(json!({"token": token, "name": name, "pc": self.pc_name()}))
            }
            Confirm::Wrong { left } => Err((format!("wrong code ({left} tries left)"), left)),
            Confirm::Unknown => Err(("that pairing request has expired; ask again".into(), 0)),
        }
    }

    fn device(&self, token: &str) -> Option<PairedDevice> {
        lock(&self.inner.cfg).device(token).cloned()
    }

    /// Publishes the status (from the UI thread, at most ~15 Hz is pushed). The hub adds the
    /// remote recorders, the browsers and its warning.
    pub fn publish(&self, mut status: RigStatus) {
        status.remotes = self.remotes();
        status.clients = lock(&self.inner.clients).iter().map(|(_, n)| n.clone()).collect();
        if status.warning.is_none() {
            status.warning = lock(&self.inner.warning).clone();
        }
        let mut s = lock(&self.inner.status);
        if s.1 == status {
            return;
        }
        let text = serde_json::to_string(&status).unwrap_or_else(|_| "{}".into());
        *s = (s.0.wrapping_add(1), status, Arc::new(text));
        drop(s);
        self.inner.changed.notify_all();
    }

    /// The browsers watching the status feed now.
    pub fn clients(&self) -> Vec<String> {
        lock(&self.inner.clients).iter().map(|(_, n)| n.clone()).collect()
    }

    /// The last status published.
    pub fn status(&self) -> RigStatus {
        lock(&self.inner.status).1.clone()
    }

    pub fn set_warning(&self, w: Option<String>) {
        *lock(&self.inner.warning) = w;
    }

    /// The booth's transport for its followers; pushed to them at once when it changes.
    pub fn set_transport(&self, t: BoothTransport) {
        let mut cur = lock(&self.inner.transport);
        if cur.1 == t {
            return;
        }
        *cur = (cur.0.wrapping_add(1), t);
        drop(cur);
        self.inner.transport_changed.notify_all();
    }

    /// The booth's transport, with the playhead as of the last [`Hub::set_position`].
    pub fn transport(&self) -> BoothTransport {
        let mut t = lock(&self.inner.transport).1.clone();
        t.position = *lock(&self.inner.position);
        t
    }

    /// The booth's playhead now (each frame; nothing is pushed for it).
    pub fn set_position(&self, secs: f64) {
        *lock(&self.inner.position) = secs;
    }

    /// The remote recorders: connected ones, and paired followers away now.
    pub fn remotes(&self) -> Vec<RemoteStatus> {
        let links = lock(&self.inner.links).clone();
        let followers: Vec<String> = lock(&self.inner.cfg).paired.iter().filter(|d| d.follows).map(|d| d.name.clone()).collect();
        let mut out: Vec<RemoteStatus> = Vec::new();
        for l in &links {
            let state = if !l.connected {
                "not reachable".to_string()
            } else if l.landed && l.report.state != "recording" {
                "take received".to_string()
            } else {
                l.report.state.clone()
            };
            let notes = l.report.notes;
            let mut parts: Vec<String> = Vec::new();
            if !l.report.ports.is_empty() {
                parts.push(format!("MIDI · {} notes", group_digits(notes)));
            }
            if !l.report.inputs.is_empty() {
                parts.push(format!("Audio · {}", l.report.inputs.join(", ")));
            }
            if let Some(c) = &l.report.check {
                parts.push(c.clone());
            }
            if l.report.clips > 0 {
                parts.push(format!("{} clip{}", l.report.clips, if l.report.clips == 1 { "" } else { "s" }));
            }
            if l.connected && l.streaming {
                parts.push("hears the mix".into());
            }
            let receiving = self.receiving(&l.name);
            if let Some(p) = receiving {
                parts.push(format!("Receiving take {:.0}%", p * 100.0));
            }
            let detail = parts.join(" · ");
            let state = if receiving.is_some() && state != "recording" { "transferring".to_string() } else { state };
            out.push(RemoteStatus {
                name: l.name.clone(),
                warning: !l.connected || state == "waiting to transfer" || l.report.clips > 0,
                state,
                detail,
                disk_free: l.report.disk_free,
                last_seen: (!l.connected).then(|| l.last_seen.elapsed().as_secs_f64()),
            });
        }
        for f in followers {
            if !out.iter().any(|r| r.name == f) {
                out.push(RemoteStatus { name: f, state: "not reachable".into(), warning: true, ..RemoteStatus::default() });
            }
        }
        out
    }

    /// Remote recorders expected (paired followers) that are not connected now.
    pub fn missing_followers(&self) -> Vec<String> {
        self.remotes().into_iter().filter(|r| r.state == "not reachable").map(|r| r.name).collect()
    }

    /// The remote ports the booth offers as track inputs (`Remote: <PC> › <port>`).
    pub fn remote_ports(&self) -> Vec<String> {
        lock(&self.inner.cfg).remote_ports.clone()
    }

    /// The remote audio inputs the booth offers as audio track inputs (`Remote: <PC> › <input>`).
    pub fn remote_inputs(&self) -> Vec<String> {
        lock(&self.inner.cfg).remote_inputs.clone()
    }

    /// The live sketches (booth tracks on a remote port draw them while recording).
    pub fn sketches(&self) -> Vec<Sketch> {
        lock(&self.inner.sketches).clone()
    }

    /// When a remote port last sent something (the activity light).
    pub fn activity(&self, port: &str) -> Option<Duration> {
        lock(&self.inner.sketches).iter().find(|s| s.port == port).map(|s| s.last.elapsed())
    }

    /// Clears the sketches of `pc` (its exact take landed), or all.
    pub fn clear_sketches(&self, pc: Option<&str>) {
        lock(&self.inner.sketches).retain(|s| pc.is_some_and(|p| s.pc != p));
    }

    /// The live levels of remote audio inputs.
    pub fn audio_sketches(&self) -> Vec<AudioSketch> {
        lock(&self.inner.audio).clone()
    }

    /// The live level of one remote audio input (`Remote: <PC> › <input>`), 0..1, while it
    /// sends (half a second after the last, nothing).
    pub fn audio_level(&self, port: &str) -> Option<f32> {
        lock(&self.inner.audio).iter().find(|s| s.port == port).filter(|s| s.last.elapsed() < Duration::from_millis(500)).map(|s| s.level)
    }

    /// Clears the live levels of `pc` (its take landed), or all.
    pub fn clear_audio(&self, pc: Option<&str>) {
        lock(&self.inner.audio).retain(|s| pc.is_some_and(|p| s.pc != p));
    }

    /// The clips of landed remote audio takes: (port, booth timeline seconds).
    pub fn clip_marks(&self) -> Vec<(String, f64)> {
        lock(&self.inner.clip_marks).clone()
    }

    /// Marks clips of a landed take on `port`.
    pub fn add_clip_marks(&self, port: &str, at: &[f64]) {
        let mut m = lock(&self.inner.clip_marks);
        for a in at.iter().filter(|a| a.is_finite()) {
            if m.len() < 10_000 {
                m.push((port.to_string(), *a));
            }
        }
    }

    /// The player's tap on the main mix, and the session rate (once a frame from the UI thread;
    /// `None` without a player).
    /// The PCs mixing this one remotely now (their names).
    pub fn mixers(&self) -> Vec<String> {
        lock(&self.inner.mixers).iter().map(|(_, n)| n.clone()).collect()
    }

    /// Publishes the session for the mixers: only the pieces that changed go out.
    pub fn publish_mix(&self, p: crate::mixer::Pieces) {
        let mut m = lock(&self.inner.mix);
        let seq = m.seq + 1;
        let mut changed = false;
        if *m.rest.1 != p.rest {
            m.rest = (seq, Arc::new(p.rest));
            changed = true;
        }
        let order = json!(p.order).to_string();
        if *m.order.1 != order {
            m.order = (seq, Arc::new(order));
            changed = true;
        }
        let old = std::mem::take(&mut m.tracks);
        for (id, text) in p.tracks {
            match old.iter().find(|(i, _, _)| *i == id) {
                Some((_, v, t)) if **t == text => m.tracks.push((id, *v, Arc::clone(t))),
                _ => {
                    m.tracks.push((id, seq, Arc::new(text)));
                    changed = true;
                }
            }
        }
        changed |= old.len() != m.tracks.len();
        if changed {
            m.seq = seq;
            drop(m);
            self.inner.mix_changed.notify_all();
        }
    }

    /// Publishes the meters and transport for the mixers.
    pub fn publish_meters(&self, f: &crate::mixer::MeterFrame) {
        let line = json!({"event": "mix.meters", "params": f}).to_string();
        let mut m = lock(&self.inner.mix);
        let seq = m.meters.0 + 1;
        m.meters = (seq, Arc::new(line));
        drop(m);
        self.inner.mix_changed.notify_all();
    }

    /// Pushes the session's pieces (those changed since last sent, then `mix.rev`) and the
    /// meters to one mixer while it is connected.
    fn push_mix(&self, out: &Arc<Mutex<TcpStream>>, done: &AtomicBool, stop: &AtomicBool) {
        let mut sent_rest = 0;
        let mut sent_order = 0;
        let mut sent_tracks: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
        let mut sent_meters = 0;
        let mut seen = u64::MAX;
        loop {
            let (lines, meters) = {
                let m = lock(&self.inner.mix);
                let (m, _) = self
                    .inner
                    .mix_changed
                    .wait_timeout_while(m, Duration::from_secs(5), |m| {
                        m.seq == seen && m.meters.0 == sent_meters && !done.load(Ordering::SeqCst) && !stop.load(Ordering::SeqCst)
                    })
                    .unwrap_or_else(PoisonError::into_inner);
                let mut lines: Vec<String> = Vec::new();
                if m.seq != seen && m.seq > 0 {
                    seen = m.seq;
                    if m.rest.0 != sent_rest {
                        sent_rest = m.rest.0;
                        lines.push(format!("{{\"event\":\"mix.rest\",\"params\":{{\"rest\":{}}}}}", m.rest.1));
                    }
                    for (id, v, t) in &m.tracks {
                        if sent_tracks.get(id) != Some(v) {
                            sent_tracks.insert(*id, *v);
                            lines.push(format!("{{\"event\":\"mix.track\",\"params\":{{\"id\":{id},\"track\":{t}}}}}"));
                        }
                    }
                    if m.order.0 != sent_order {
                        sent_order = m.order.0;
                        sent_tracks.retain(|id, _| m.tracks.iter().any(|(i, _, _)| i == id));
                        lines.push(format!("{{\"event\":\"mix.order\",\"params\":{{\"order\":{}}}}}", m.order.1));
                    }
                    lines.push(format!("{{\"event\":\"mix.rev\",\"params\":{{\"seq\":{}}}}}", m.seq));
                }
                let meters = (m.meters.0 != sent_meters && m.meters.0 > 0).then(|| {
                    sent_meters = m.meters.0;
                    Arc::clone(&m.meters.1)
                });
                (lines, meters)
            };
            if done.load(Ordering::SeqCst) || stop.load(Ordering::SeqCst) {
                break;
            }
            let mut w = lock(out);
            let mut ok = true;
            for l in lines.iter().map(String::as_str).chain(meters.as_deref().map(String::as_str)) {
                ok = writeln!(w, "{l}").is_ok();
                if !ok {
                    break;
                }
            }
            if lines.is_empty() && meters.is_none() {
                ok = writeln!(w, "{{\"event\":\"tick\"}}").is_ok();
            }
            if !ok || w.flush().is_err() {
                break;
            }
        }
    }

    pub fn set_tap(&self, tap: Option<Arc<soundcraft_playback::stream::MixTap>>, rate: u32) {
        let mut cur = lock(&self.inner.tap);
        let same = match (&*cur, &tap) {
            (Some((a, r)), Some(b)) => Arc::ptr_eq(a, b) && *r == rate,
            (None, None) => true,
            _ => false,
        };
        if !same {
            if let Some((old, _)) = &*cur {
                old.set_on(false);
            }
            *cur = tap.map(|t| (t, rate));
        }
    }

    /// The remote recorders hearing the mix now.
    pub fn stream_listeners(&self) -> usize {
        lock(&self.inner.stream_subs).len()
    }

    /// A link wants the mix (or not): the stream thread starts with the first.
    fn subscribe(&self, link: u64, out: &Arc<Mutex<TcpStream>>, on: bool) {
        {
            let mut subs = lock(&self.inner.stream_subs);
            subs.retain(|(l, _)| *l != link);
            if on {
                subs.push((link, Arc::clone(out)));
            }
        }
        for l in lock(&self.inner.links).iter_mut().filter(|l| l.id == link) {
            l.streaming = on;
        }
        if on && !self.inner.streaming.swap(true, Ordering::SeqCst) {
            let weak = Arc::downgrade(&self.inner);
            let spawned = std::thread::Builder::new().name("freedawm-mix-stream".into()).spawn(move || stream_loop(&weak));
            if spawned.is_err() {
                self.inner.streaming.store(false, Ordering::SeqCst);
            }
        }
    }

    /// Where remote audio goes and in which format (once a frame from the UI thread).
    pub fn set_project(&self, p: ProjectInfo) {
        let mut cur = lock(&self.inner.project);
        if *cur != p {
            *cur = p;
        }
    }

    pub fn project(&self) -> ProjectInfo {
        lock(&self.inner.project).clone()
    }

    /// Where arriving transfer files are kept: the project's `Remote Takes`, else
    /// `Remote Inbox` next to the network settings (or in the temp folder).
    pub fn inbox(&self) -> PathBuf {
        if let Some(f) = &lock(&self.inner.project).folder {
            return f.join(crate::follower::PROJECT_TAKES);
        }
        self.fallback_dir().join("Remote Inbox")
    }

    fn fallback_dir(&self) -> PathBuf {
        self.inner
            .cfg_path
            .as_deref()
            .and_then(std::path::Path::parent)
            .map_or_else(|| std::env::temp_dir().join("FreeDAWm"), std::path::Path::to_path_buf)
    }

    /// Where landed remote audio files go: the project's `Audio Files`, else next to the inbox.
    pub fn audio_dir(&self) -> PathBuf {
        match &lock(&self.inner.project).folder {
            Some(f) => f.join("Audio Files"),
            None => self.fallback_dir().join("Remote Inbox").join("Audio Files"),
        }
    }

    /// How far a take from `pc` has arrived (0..1), while one does.
    pub fn receiving(&self, pc: &str) -> Option<f64> {
        lock(&self.inner.uploads).iter().find(|u| u.pc == pc && u.have < u.bytes).map(|u| u.have as f64 / u.bytes.max(1) as f64)
    }

    /// A take from `pc` landed (the page shows "take received").
    pub fn note_landed(&self, pc: &str, id: &str) {
        for l in lock(&self.inner.links).iter_mut().filter(|l| l.name == pc) {
            l.landed = true;
        }
        self.update_config(|c| c.note_landed(id));
    }

    pub fn already_landed(&self, id: &str) -> bool {
        lock(&self.inner.cfg).landed.iter().any(|l| l == id)
    }

    /// The port this hub listens on, while it does.
    pub fn listening_port(&self) -> Option<u16> {
        lock(&self.inner.listening).as_ref().map(|l| l.port)
    }

    /// Starts listening on all interfaces at `port` (0: any free port). Stops a listener there was.
    pub fn listen(&self, port: u16) -> Result<u16, String> {
        self.stop();
        let listener = TcpListener::bind(("0.0.0.0", port)).map_err(|e| format!("cannot listen on port {port}: {e}"))?;
        let actual = listener.local_addr().map(|a| a.port()).unwrap_or(port);
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        *lock(&self.inner.listening) = Some(Listening { port: actual, stop: Arc::clone(&stop) });
        let hub = self.clone();
        std::thread::Builder::new()
            .name("freedawm-net-accept".into())
            .spawn(move || hub.accept_loop(listener, stop, false))
            .map_err(|e| e.to_string())?;
        log::info!("network: listening on port {actual}");
        Ok(actual)
    }

    /// The listener the internet bridge connects to ([`crate::internet::Internet::serve_to`]):
    /// loopback only, any free port; JSON lines only (no page), and only pairing by invite
    /// (nobody across the internet sees this PC's screen for a code). Kept until
    /// [`Hub::stop_internet`]; returns its port.
    pub fn listen_internet(&self) -> Result<u16, String> {
        if let Some(l) = lock(&self.inner.internet).as_ref() {
            return Ok(l.port);
        }
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("cannot listen for the internet bridge: {e}"))?;
        let port = listener.local_addr().map(|a| a.port()).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        *lock(&self.inner.internet) = Some(Listening { port, stop: Arc::clone(&stop) });
        let hub = self.clone();
        std::thread::Builder::new()
            .name("freedawm-net-accept-internet".into())
            .spawn(move || hub.accept_loop(listener, stop, true))
            .map_err(|e| e.to_string())?;
        Ok(port)
    }

    /// The internet bridge's listener port, while there is one.
    pub fn internet_port(&self) -> Option<u16> {
        lock(&self.inner.internet).as_ref().map(|l| l.port)
    }

    pub fn stop_internet(&self) {
        if let Some(l) = lock(&self.inner.internet).take() {
            l.stop.store(true, Ordering::SeqCst);
            self.inner.changed.notify_all();
            self.inner.transport_changed.notify_all();
        }
    }

    /// A one-time invite secret, valid for [`INVITE_MINUTES`]; see [`crate::internet::Invite`].
    pub fn new_invite_secret(&self) -> String {
        let secret = new_token();
        let until = self.now_s() + INVITE_MINUTES * 60.0;
        let mut inv = lock(&self.inner.invites);
        let now = self.now_s();
        inv.retain(|(_, t)| *t > now);
        inv.push((secret.clone(), until));
        secret
    }

    /// Invites waiting (not used, not expired): seconds each has left.
    pub fn open_invites(&self) -> Vec<f64> {
        let now = self.now_s();
        lock(&self.inner.invites).iter().filter(|(_, t)| *t > now).map(|(_, t)| t - now).collect()
    }

    /// For tests: invites expire `secs` from now.
    pub fn age_invites(&self, secs: f64) {
        for (_, t) in lock(&self.inner.invites).iter_mut() {
            *t -= secs;
        }
    }

    /// `pair.invite {secret, name}`: an invite's secret, once, before it expires, pairs `name`.
    fn pair_invite(&self, secret: &str, name: &str) -> Result<Value, String> {
        let now = self.now_s();
        let ok = {
            let mut inv = lock(&self.inner.invites);
            let found = inv.iter().position(|(s, t)| !secret.is_empty() && s == secret && *t > now);
            if let Some(i) = found {
                inv.remove(i);
            }
            found.is_some()
        };
        if !ok {
            return Err("that invite has expired or was already used: ask for a new one".into());
        }
        let name = crate::pairing::clean_name(name);
        let token = new_token();
        let dev = PairedDevice { name: name.clone(), kind: "pc".into(), token: token.clone(), follows: false };
        self.update_config(|c| c.paired.push(dev));
        log::info!("network: {name} paired over the internet with an invite");
        Ok(json!({"token": token, "name": name, "pc": self.pc_name()}))
    }

    /// Stops listening (open connections end within a few seconds).
    pub fn stop(&self) {
        if let Some(l) = lock(&self.inner.listening).take() {
            l.stop.store(true, Ordering::SeqCst);
            self.inner.changed.notify_all();
            self.inner.transport_changed.notify_all();
        }
    }

    fn stopped(&self, stop: &AtomicBool) -> bool {
        stop.load(Ordering::SeqCst)
    }

    fn accept_loop(&self, listener: TcpListener, stop: Arc<AtomicBool>, internet: bool) {
        while !self.stopped(&stop) {
            match listener.accept() {
                Ok((stream, addr)) => {
                    if self.inner.open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                        self.inner.open.fetch_sub(1, Ordering::SeqCst);
                        continue;
                    }
                    let hub = self.clone();
                    let stop = Arc::clone(&stop);
                    let spawned = std::thread::Builder::new().name("freedawm-net-conn".into()).spawn(move || {
                        hub.serve(stream, addr, &stop, internet);
                        hub.inner.open.fetch_sub(1, Ordering::SeqCst);
                    });
                    if spawned.is_err() {
                        self.inner.open.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                Err(e) => {
                    log::warn!("network accept: {e}");
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }

    fn serve(&self, stream: TcpStream, addr: SocketAddr, stop: &Arc<AtomicBool>, internet: bool) {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(LINK_TIMEOUT));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let Ok(read) = stream.try_clone() else { return };
        let mut reader = BufReader::new(read);
        let first = match reader.fill_buf() {
            Ok(b) if !b.is_empty() => b.first().copied().unwrap_or(0),
            _ => return,
        };
        if first == b'{' {
            self.serve_lines(reader, stream, addr, stop, internet);
        } else if internet {
            // No service remote page over the internet.
        } else {
            self.serve_http(reader, stream, stop);
        }
    }

    // ---- HTTP: the service remote page ----

    fn serve_http(&self, mut reader: BufReader<TcpStream>, mut out: TcpStream, stop: &Arc<AtomicBool>) {
        let Some(req) = read_http(&mut reader) else {
            let _ = respond(&mut out, 400, "text/plain", b"bad request");
            return;
        };
        let token = req.header("x-token").map(str::to_string).or_else(|| req.query("token")).unwrap_or_default();
        let body: Value = serde_json::from_slice(&req.body).unwrap_or(json!({}));
        let r = match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/" | "/index.html") => respond(&mut out, 200, "text/html; charset=utf-8", PAGE.as_bytes()),
            ("GET", "/remote.js") => respond(&mut out, 200, "text/javascript; charset=utf-8", SCRIPT.as_bytes()),
            ("GET", "/remote.css") => respond(&mut out, 200, "text/css; charset=utf-8", STYLE.as_bytes()),
            ("POST", "/api/pair/request") => {
                let name = body.get("name").and_then(Value::as_str).unwrap_or("Browser");
                match self.pair_request(name, "browser") {
                    Ok(v) => respond_json(&mut out, 200, &v),
                    Err(e) => respond_json(&mut out, 429, &json!({"error": e})),
                }
            }
            ("POST", "/api/pair/confirm") => {
                let id = body.get("request").and_then(Value::as_str).unwrap_or("");
                let code = body.get("code").and_then(Value::as_str).unwrap_or("");
                match self.pair_confirm(id, code) {
                    Ok(v) => respond_json(&mut out, 200, &v),
                    Err((e, left)) => respond_json(&mut out, 403, &json!({"error": e, "left": left})),
                }
            }
            (_, path) if path.starts_with("/api/") => {
                let Some(dev) = self.device(&token) else {
                    let _ = respond_json(&mut out, 401, &json!({"error": "not paired", "pair": true}));
                    return;
                };
                match (req.method.as_str(), path) {
                    ("GET", "/api/status") => respond_json(&mut out, 200, &self.status_value()),
                    ("GET", "/api/events") => {
                        self.serve_events(out, &dev, stop);
                        return;
                    }
                    ("POST", "/api/action") => {
                        let reply = match serde_json::from_value::<RemoteAction>(body) {
                            Ok(action) => self.action(action, &dev.name),
                            Err(e) => Err(format!("unknown action: {e}")),
                        };
                        match reply {
                            Ok(v) => respond_json(&mut out, 200, &json!({"ok": true, "result": v})),
                            Err(e) => respond_json(&mut out, 409, &json!({"ok": false, "error": e})),
                        }
                    }
                    _ => respond(&mut out, 404, "text/plain", b"not found"),
                }
            }
            _ => respond(&mut out, 404, "text/plain", b"not found"),
        };
        let _ = r;
    }

    fn status_value(&self) -> Value {
        let s = lock(&self.inner.status);
        serde_json::from_str(&s.2).unwrap_or(json!({}))
    }

    /// Runs a service remote action through the UI thread.
    pub fn action(&self, action: RemoteAction, device: &str) -> Result<Value, String> {
        let (tx, rx) = channel();
        if !self.send(HubRequest::Action { action, device: device.to_string(), reply: tx }) {
            return Err("the app is not taking requests".into());
        }
        rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "the app did not answer".to_string())?
    }

    /// The status feed as server-sent events, until the browser goes or the hub stops.
    fn serve_events(&self, mut out: TcpStream, dev: &PairedDevice, stop: &Arc<AtomicBool>) {
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\nAccess-Control-Allow-Origin: *\r\n\r\n";
        if out.write_all(head.as_bytes()).is_err() {
            return;
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        lock(&self.inner.clients).push((id, dev.name.clone()));
        let mut seen = u64::MAX;
        let mut last_push = Instant::now() - STATUS_EVERY;
        loop {
            if self.stopped(stop) {
                break;
            }
            let text = {
                let s = lock(&self.inner.status);
                let (s, _) = self
                    .inner
                    .changed
                    .wait_timeout_while(s, Duration::from_secs(10), |s| s.0 == seen && !self.stopped(stop))
                    .unwrap_or_else(PoisonError::into_inner);
                (s.0 != seen).then_some(s.0)
            };
            let wrote = match text {
                Some(seq) => {
                    seen = seq;
                    // At most ~15 pushes a second: the latest status after the wait.
                    let wait = STATUS_EVERY.saturating_sub(last_push.elapsed());
                    if !wait.is_zero() {
                        std::thread::sleep(wait);
                    }
                    last_push = Instant::now();
                    let latest = Arc::clone(&lock(&self.inner.status).2);
                    out.write_all(format!("data: {latest}\n\n").as_bytes())
                }
                // Nothing changed for a while: the same status again (the page knows the booth is there).
                None => {
                    let latest = Arc::clone(&lock(&self.inner.status).2);
                    out.write_all(format!("data: {latest}\n\n").as_bytes())
                }
            };
            if wrote.is_err() || out.flush().is_err() {
                break;
            }
        }
        lock(&self.inner.clients).retain(|(i, _)| *i != id);
    }

    // ---- JSON lines: paired PCs and remote recorders ----

    fn serve_lines(&self, mut reader: BufReader<TcpStream>, out: TcpStream, addr: SocketAddr, stop: &Arc<AtomicBool>, internet: bool) {
        let out = Arc::new(Mutex::new(out));
        let mut buf = Vec::new();
        let mut who: Option<PairedDevice> = None;
        let mut link: Option<u64> = None;
        let mut pusher: Option<Arc<AtomicBool>> = None;
        // A mixer connection: its id (for the stream), and its pusher.
        let mut mixer: Option<u64> = None;
        loop {
            if self.stopped(stop) {
                break;
            }
            match read_line(&mut reader, &mut buf, MAX_LINE) {
                Ok(true) => {}
                Ok(false) | Err(_) => break,
            }
            let Ok(text) = std::str::from_utf8(&buf) else { break };
            if text.trim().is_empty() {
                continue;
            }
            // Arrival time on the booth's clock, before anything else (a sync ping).
            let arrived = self.clock_now();
            let msg: Value = match serde_json::from_str(text) {
                Ok(v) => v,
                Err(e) => {
                    write_line(&out, &json!({"ok": false, "error": format!("bad JSON: {e}")}));
                    continue;
                }
            };
            let id = msg.get("id").cloned();
            let method = msg.get("method").and_then(Value::as_str).unwrap_or("").to_string();
            let params = msg.get("params").cloned().unwrap_or(json!({}));
            if let Some(l) = link {
                self.touch_link(l);
            }
            let reply: Option<Value> = match method.as_str() {
                "pair.request" | "pair.confirm" if internet => {
                    Some(json!({"ok": false, "error": "over the internet, pair with an invite from the booth", "pair": true}))
                }
                "pair.invite" => Some(answer(self.pair_invite(
                    params.get("secret").and_then(Value::as_str).unwrap_or(""),
                    params.get("name").and_then(Value::as_str).unwrap_or("PC"),
                ))),
                "pair.request" => Some(answer(self.pair_request(params.get("name").and_then(Value::as_str).unwrap_or("PC"), "pc"))),
                "pair.confirm" => Some(answer(
                    self.pair_confirm(
                        params.get("request").and_then(Value::as_str).unwrap_or(""),
                        params.get("code").and_then(Value::as_str).unwrap_or(""),
                    )
                    .map_err(|(e, _)| e),
                )),
                "hello" => {
                    let token = params.get("token").and_then(Value::as_str).unwrap_or("");
                    match self.device(token) {
                        None => Some(json!({"ok": false, "error": "not paired", "pair": true})),
                        Some(dev) => {
                            let role = params.get("role").and_then(Value::as_str).unwrap_or("");
                            let follower = role == "follower";
                            if role == "mixer" && mixer.is_none() {
                                let m = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
                                mixer = Some(m);
                                lock(&self.inner.mixers).push((m, dev.name.clone()));
                                let flag = Arc::new(AtomicBool::new(false));
                                pusher = Some(Arc::clone(&flag));
                                let hub = self.clone();
                                let out = Arc::clone(&out);
                                let stop = Arc::clone(stop);
                                let _ =
                                    std::thread::Builder::new().name("freedawm-mixer-push".into()).spawn(move || hub.push_mix(&out, &flag, &stop));
                                self.wake();
                            }
                            if follower {
                                let report: FollowerReport =
                                    params.get("report").cloned().and_then(|r| serde_json::from_value(r).ok()).unwrap_or_default();
                                let l = self.open_link(&dev, report);
                                link = Some(l);
                                let flag = Arc::new(AtomicBool::new(false));
                                pusher = Some(Arc::clone(&flag));
                                let hub = self.clone();
                                let out = Arc::clone(&out);
                                let stop = Arc::clone(stop);
                                let _ = std::thread::Builder::new()
                                    .name("freedawm-net-push".into())
                                    .spawn(move || hub.push_transport(&out, &flag, &stop));
                            }
                            log::info!(
                                "network: {} connected from {addr}{}",
                                dev.name,
                                if follower {
                                    " as a remote recorder"
                                } else if mixer.is_some() {
                                    " to mix remotely"
                                } else {
                                    ""
                                }
                            );
                            let r = json!({"ok": true, "result": {"pc": self.pc_name(), "name": dev.name, "transport": self.transport()}});
                            who = Some(dev);
                            Some(r)
                        }
                    }
                }
                _ if who.is_none() => Some(json!({"ok": false, "error": "not paired: send pair.request / pair.confirm, then hello", "pair": true})),
                // A mixer may change the mix and run the transport, nothing else.
                m if mixer.is_some() && !mixer_allows(m, &params) => Some(json!({"ok": false, "error": format!("a remote mixer can't run `{m}`")})),
                "sync.ping" => {
                    let now = self.clock_now();
                    match (arrived, now) {
                        (Some(b1), Some(b2)) => Some(json!({"ok": true, "result": {"b1": b1, "b2": b2}})),
                        _ => Some(json!({"ok": false, "error": "the booth is stopped"})),
                    }
                }
                "follower.report" => {
                    if let (Some(l), Ok(r)) = (link, serde_json::from_value::<FollowerReport>(params)) {
                        self.update_link(l, r);
                    }
                    None
                }
                "feed" => {
                    if let Some(dev) = &who {
                        self.feed(&dev.name, &params);
                    }
                    None
                }
                "feed.audio" => {
                    if let Some(dev) = &who {
                        self.feed_audio(&dev.name, &params);
                    }
                    None
                }
                "stream.subscribe" => {
                    let on = params.get("on").and_then(Value::as_bool).unwrap_or(true);
                    match link.or(mixer) {
                        Some(l) => {
                            self.subscribe(l, &out, on);
                            Some(json!({"ok": true, "result": {"on": on}}))
                        }
                        None => Some(json!({"ok": false, "error": "only a remote recorder or a remote mixer hears the mix"})),
                    }
                }
                "take.offer" => Some(answer(who.as_ref().map_or(Err("not paired".into()), |d| self.take_offer(&d.name, &params)))),
                "take.chunk" => Some(answer(self.take_chunk(&params))),
                "take.transfer" => match serde_json::from_value::<TakeEnvelope>(params) {
                    Ok(t) => Some(answer(self.receive_take(t))),
                    Err(e) => Some(json!({"ok": false, "error": format!("bad take: {e}")})),
                },
                _ => {
                    let (tx, rx) = channel();
                    if !self.send(HubRequest::Control { method, params, reply: tx }) {
                        break;
                    }
                    Some(rx.recv_timeout(Duration::from_secs(90)).unwrap_or_else(|_| json!({"ok": false, "error": "timeout"})))
                }
            };
            if let (Some(mut r), Some(id)) = (reply, id) {
                if let Some(o) = r.as_object_mut() {
                    o.insert("id".into(), id);
                }
                if !write_line(&out, &r) {
                    break;
                }
            }
        }
        if let Some(p) = pusher {
            p.store(true, Ordering::SeqCst);
            self.inner.transport_changed.notify_all();
        }
        if let Some(l) = link {
            self.subscribe(l, &out, false);
            self.close_link(l);
        }
        if let Some(m) = mixer {
            self.subscribe(m, &out, false);
            lock(&self.inner.mixers).retain(|(id, _)| *id != m);
            self.inner.mix_changed.notify_all();
            self.wake();
        }
    }

    fn receive_take(&self, t: TakeEnvelope) -> Result<Value, String> {
        if self.already_landed(&t.id) {
            return Ok(json!({"landed": t.id, "again": true}));
        }
        let prepared = if t.audio().is_some() { Some(self.prepare_audio(&t)?) } else { None };
        let (tx, rx) = channel();
        let id = t.id.clone();
        if !self.send(HubRequest::Take { take: Box::new(t), prepared, reply: tx }) {
            return Err("the booth is not taking takes".into());
        }
        rx.recv_timeout(Duration::from_secs(60)).map_err(|_| "the booth did not answer".to_string())??;
        Ok(json!({"landed": id}))
    }

    /// `take.offer {id, file, bytes, hash}`: a transfer file is coming; answers how much of it is
    /// here already (`have`), so a transfer cut off resumes where it stopped.
    fn take_offer(&self, pc: &str, p: &Value) -> Result<Value, String> {
        let id = p.get("id").and_then(Value::as_str).ok_or("no take id")?.to_string();
        let file = p.get("file").and_then(Value::as_str).ok_or("no file")?;
        let bytes = p.get("bytes").and_then(Value::as_u64).ok_or("no size")?;
        let hash = p.get("hash").and_then(Value::as_str).unwrap_or("").to_string();
        if bytes > MAX_TAKE_BYTES {
            return Err("that take is too large".into());
        }
        if self.already_landed(&id) {
            return Ok(json!({"have": bytes, "landed": true}));
        }
        let dir = self.inbox();
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let done = dir.join(file_name(file));
        let part = dir.join(format!("{}.part", file_name(file)));
        let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        if std::fs::metadata(&done).is_ok_and(|m| m.len() == bytes) {
            have = bytes;
        } else if have > bytes {
            let _ = std::fs::remove_file(&part);
            have = 0;
        }
        let mut ups = lock(&self.inner.uploads);
        ups.retain(|u| u.id != id);
        ups.push(Upload { id, pc: pc.to_string(), bytes, have, hash, part, done });
        Ok(json!({"have": have}))
    }

    /// `take.chunk {id, offset, data}` (base64): appended when it starts where the file ends;
    /// answers `have` either way (a chunk out of order makes the sender go back there).
    fn take_chunk(&self, p: &Value) -> Result<Value, String> {
        let id = p.get("id").and_then(Value::as_str).ok_or("no take id")?;
        let offset = p.get("offset").and_then(Value::as_u64).ok_or("no offset")?;
        let mut ups = lock(&self.inner.uploads);
        let u = ups.iter_mut().find(|u| u.id == id).ok_or("offer the take first")?;
        if offset != u.have || u.have >= u.bytes {
            return Ok(json!({"have": u.have}));
        }
        let data = crate::audio::b64_decode(p.get("data").and_then(Value::as_str).unwrap_or("")).ok_or("bad chunk")?;
        let room = u.bytes.saturating_sub(u.have);
        let data = data.get(..usize::try_from(room).unwrap_or(usize::MAX).min(data.len())).unwrap_or(&data);
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&u.part).map_err(|e| format!("{}: {e}", u.part.display()))?;
        f.write_all(data).and_then(|_| f.flush()).map_err(|e| format!("{}: {e}", u.part.display()))?;
        u.have = u.have.saturating_add(data.len() as u64);
        if u.have >= u.bytes {
            let bytes = std::fs::read(&u.part).map_err(|e| e.to_string())?;
            let got = crate::audio::hash(&bytes);
            if !u.hash.is_empty() && got != u.hash {
                let _ = std::fs::remove_file(&u.part);
                u.have = 0;
                return Err("the take arrived damaged; sending it again".into());
            }
            std::fs::rename(&u.part, &u.done).map_err(|e| e.to_string())?;
        }
        Ok(json!({"have": u.have}))
    }

    /// Makes an arrived audio take ready to land (see [`PreparedAudio`]).
    fn prepare_audio(&self, t: &TakeEnvelope) -> Result<PreparedAudio, String> {
        let a = t.audio().ok_or("not an audio take")?;
        let src = self.inbox().join(file_name(&a.file));
        let bytes = std::fs::read(&src).map_err(|_| format!("{}: the take's file has not arrived", t.pc))?;
        if bytes.len() as u64 != a.bytes || (!a.hash.is_empty() && crate::audio::hash(&bytes) != a.hash) {
            return Err(format!("{}: the take's file is incomplete", t.pc));
        }
        let ext = src.extension().and_then(|x| x.to_str()).map(str::to_string);
        let (_, buf) = soundcraft_audio_io::decode(&bytes, ext.as_deref()).map_err(|e| format!("{}: {e}", src.display()))?;
        drop(bytes);
        let proj = self.project();
        let rate = if proj.rate > 0 { proj.rate } else { buf.sample_rate };
        let bits = if proj.bits > 0 { proj.bits } else { a.bits };
        let map = a.clock_map(&t.sync);
        let al = crate::audio::align(buf, a.t0, map.as_ref(), rate);
        let wav = crate::audio::encode_landed(&al.buf, bits)?;
        let dir = self.audio_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let stem = file_name(&format!("{} {}", t.pc, a.input));
        let path = (1..10_000).map(|n| dir.join(format!("{stem}_{n:02}.wav"))).find(|p| !p.exists()).ok_or("no file name left")?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &wav).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| format!("{}: {e}", path.display()))?;
        let clips = a
            .clips
            .iter()
            .map(|c| {
                let booth = map.as_ref().map_or(*c, |m| m.map(*c));
                timeline_of(&t.anchor, booth)
            })
            .collect();
        log::info!("remote take {}: {} at {rate} Hz, drift {:.2} samples corrected", t.id, path.display(), al.drift);
        Ok(PreparedAudio { wav: path, first: al.first, converted: al.converted, drift: al.drift, clips })
    }

    /// A live level batch from `pc`: `{input, peaks: [[at, peak]…], clips: [at…]}` (booth timeline
    /// seconds).
    fn feed_audio(&self, pc: &str, params: &Value) {
        let port = remote_port_name(pc, params.get("input").and_then(Value::as_str).unwrap_or(""));
        let peaks: Vec<(f64, f32)> = params.get("peaks").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
        let clips: Vec<f64> = params.get("clips").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
        let level = params.get("level").and_then(Value::as_f64).map(|l| l as f32);
        let mut sk = lock(&self.inner.audio);
        let s = match sk.iter_mut().position(|s| s.port == port) {
            Some(i) => sk.get_mut(i),
            None => {
                sk.push(AudioSketch {
                    port: port.clone(),
                    pc: pc.to_string(),
                    peaks: Vec::new(),
                    clips: Vec::new(),
                    level: 0.0,
                    last: Instant::now(),
                });
                sk.last_mut()
            }
        };
        if let Some(s) = s {
            s.last = Instant::now();
            let room = MAX_SKETCH.saturating_sub(s.peaks.len());
            s.peaks.extend(peaks.iter().filter(|(a, p)| a.is_finite() && p.is_finite()).take(room));
            s.clips.extend(clips.into_iter().filter(|c| c.is_finite()).take(10_000usize.saturating_sub(s.clips.len())));
            s.level = level.or_else(|| peaks.last().map(|p| p.1)).filter(|l| l.is_finite()).unwrap_or(0.0).clamp(0.0, 2.0);
        }
    }

    /// Adds a live level batch for `pc`'s `input` (as `feed.audio` does; for renders and tests).
    pub fn feed_audio_sketch(&self, pc: &str, input: &str, peaks: &[(f64, f32)], clips: &[f64]) {
        self.feed_audio(pc, &json!({"input": input, "peaks": peaks, "clips": clips}));
    }

    /// Pushes the booth's transport to one follower whenever it changes (and a tick every 5 s).
    fn push_transport(&self, out: &Arc<Mutex<TcpStream>>, done: &AtomicBool, stop: &AtomicBool) {
        let mut seen = lock(&self.inner.transport).0;
        loop {
            let next = {
                let t = lock(&self.inner.transport);
                let (t, _) = self
                    .inner
                    .transport_changed
                    .wait_timeout_while(t, Duration::from_secs(5), |t| t.0 == seen && !done.load(Ordering::SeqCst) && !stop.load(Ordering::SeqCst))
                    .unwrap_or_else(PoisonError::into_inner);
                (t.0 != seen).then(|| (t.0, t.1.clone()))
            };
            if done.load(Ordering::SeqCst) || stop.load(Ordering::SeqCst) {
                break;
            }
            let msg = match next {
                Some((seq, t)) => {
                    seen = seq;
                    json!({"event": "transport", "params": t})
                }
                None => json!({"event": "tick"}),
            };
            if !write_line(out, &msg) {
                break;
            }
        }
    }

    fn open_link(&self, dev: &PairedDevice, report: FollowerReport) -> u64 {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let ports: Vec<String> = report.ports.iter().map(|p| remote_port_name(&dev.name, p)).collect();
        let inputs: Vec<String> = report.inputs.iter().map(|p| remote_port_name(&dev.name, p)).collect();
        {
            let mut links = lock(&self.inner.links);
            links.retain(|l| l.name != dev.name);
            links.push(Link { id, name: dev.name.clone(), report, connected: true, last_seen: Instant::now(), landed: false, streaming: false });
        }
        let name = dev.name.clone();
        self.update_config(|c| {
            for d in c.paired.iter_mut().filter(|d| d.name == name) {
                d.follows = true;
            }
            for p in ports {
                if !c.remote_ports.contains(&p) {
                    c.remote_ports.push(p);
                }
            }
            for p in inputs {
                if !c.remote_inputs.contains(&p) {
                    c.remote_inputs.push(p);
                }
            }
        });
        id
    }

    fn touch_link(&self, id: u64) {
        for l in lock(&self.inner.links).iter_mut().filter(|l| l.id == id) {
            l.last_seen = Instant::now();
        }
    }

    fn update_link(&self, id: u64, report: FollowerReport) {
        let mut new_ports = Vec::new();
        let mut new_inputs = Vec::new();
        for l in lock(&self.inner.links).iter_mut().filter(|l| l.id == id) {
            if report.state == "recording" {
                l.landed = false;
            }
            new_ports = report.ports.iter().map(|p| remote_port_name(&l.name, p)).collect();
            new_inputs = report.inputs.iter().map(|p| remote_port_name(&l.name, p)).collect();
            l.report = report.clone();
        }
        let missing = {
            let c = lock(&self.inner.cfg);
            new_ports.iter().any(|p| !c.remote_ports.contains(p)) || new_inputs.iter().any(|p| !c.remote_inputs.contains(p))
        };
        if missing {
            self.update_config(|c| {
                for p in new_ports {
                    if !c.remote_ports.contains(&p) {
                        c.remote_ports.push(p);
                    }
                }
                for p in new_inputs {
                    if !c.remote_inputs.contains(&p) {
                        c.remote_inputs.push(p);
                    }
                }
            });
        }
    }

    fn close_link(&self, id: u64) {
        for l in lock(&self.inner.links).iter_mut().filter(|l| l.id == id) {
            l.connected = false;
            log::info!("network: remote recorder {} disconnected", l.name);
        }
    }

    /// Adds notes to the live sketch of `pc`'s `port` (as a feed batch does; for renders).
    pub fn feed_sketch(&self, pc: &str, port: &str, notes: &[SketchNote]) {
        self.feed(pc, &json!({"port": port, "notes": notes}));
    }

    /// A live feed batch from `pc`: `{port, notes: [{at, pitch, vel}]}`.
    fn feed(&self, pc: &str, params: &Value) {
        let port = remote_port_name(pc, params.get("port").and_then(Value::as_str).unwrap_or(""));
        let notes: Vec<SketchNote> = params.get("notes").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
        let mut sk = lock(&self.inner.sketches);
        let s = match sk.iter_mut().position(|s| s.port == port) {
            Some(i) => sk.get_mut(i),
            None => {
                sk.push(Sketch { port: port.clone(), pc: pc.to_string(), notes: Vec::new(), last: Instant::now() });
                sk.last_mut()
            }
        };
        if let Some(s) = s {
            s.last = Instant::now();
            let room = MAX_SKETCH.saturating_sub(s.notes.len());
            s.notes.extend(notes.into_iter().filter(|n| n.at.is_finite()).take(room));
        }
    }
}

/// What a mixer connection may call: the mix, automation and transport commands, and reading.
fn mixer_allows(method: &str, params: &Value) -> bool {
    match method {
        "engine.execute" | "command" => {
            let id = params.get("command").or_else(|| params.get("id")).and_then(Value::as_str).unwrap_or("");
            crate::mixer::host_allows(id)
        }
        "session.inspect" | "document.inspect" | "engine.commands" | "stream.subscribe" | "sync.ping" => true,
        _ => false,
    }
}

fn answer(r: Result<Value, String>) -> Value {
    match r {
        Ok(v) => json!({"ok": true, "result": v}),
        Err(e) => json!({"ok": false, "error": e}),
    }
}

fn write_line(out: &Arc<Mutex<TcpStream>>, v: &Value) -> bool {
    let mut s = lock(out);
    writeln!(s, "{v}").is_ok() && s.flush().is_ok()
}

/// Reads one `\n`-terminated line of at most `max` bytes (newline stripped). `Ok(false)` at end
/// of stream; `Err` when too long or the read fails (a timeout included).
pub(crate) fn read_line(reader: &mut impl BufRead, buf: &mut Vec<u8>, max: usize) -> Result<bool, String> {
    buf.clear();
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    let n = reader.take(limit).read_until(b'\n', buf).map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(false);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        return Ok(true);
    }
    if buf.len() > max {
        return Err("line too long".into());
    }
    Ok(true)
}

/// One HTTP request (the head, bounded, and a body of at most [`MAX_BODY`]).
struct HttpRequest {
    method: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    fn query(&self, key: &str) -> Option<String> {
        self.query.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v.to_string()))
    }
}

fn read_http(reader: &mut BufReader<TcpStream>) -> Option<HttpRequest> {
    let mut head = Vec::new();
    let mut line = Vec::new();
    loop {
        if !read_line(reader, &mut line, MAX_HEAD).ok()? {
            return None;
        }
        if line.is_empty() {
            break;
        }
        head.push(String::from_utf8(line.clone()).ok()?);
        if head.iter().map(String::len).sum::<usize>() > MAX_HEAD {
            return None;
        }
    }
    let first = head.first()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let (path, query) = target.split_once('?').map_or((target.clone(), String::new()), |(p, q)| (p.to_string(), q.to_string()));
    let headers: Vec<(String, String)> =
        head.iter().skip(1).filter_map(|h| h.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))).collect();
    let len = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse::<usize>().ok()).unwrap_or(0);
    if len > MAX_BODY {
        return None;
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body).ok()?;
    Some(HttpRequest { method, path, query, headers, body })
}

fn respond(out: &mut TcpStream, code: u16, kind: &str, body: &[u8]) -> std::io::Result<()> {
    let reason = match code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        429 => "Too Many Requests",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    out.write_all(head.as_bytes())?;
    out.write_all(body)?;
    out.flush()
}

fn respond_json(out: &mut TcpStream, code: u16, v: &Value) -> std::io::Result<()> {
    respond(out, code, "application/json", v.to_string().as_bytes())
}

/// The remote mix stream's thread: reads the tap, encodes, sends to every link hearing it; the
/// tap is on only while someone listens. Ends with the hub.
fn stream_loop(weak: &std::sync::Weak<Inner>) {
    let mut enc: Option<(u32, crate::stream::MixEncoder)> = None;
    let mut blocks = Vec::new();
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let subs: Vec<(u64, Arc<Mutex<TcpStream>>)> = lock(&inner.stream_subs).clone();
        let tap = lock(&inner.tap).clone();
        match (&tap, subs.is_empty()) {
            (Some((t, rate)), false) => {
                if !t.is_on() {
                    t.clear();
                    t.set_on(true);
                }
                if enc.as_ref().is_none_or(|(r, _)| r != rate) {
                    enc = Some((*rate, crate::stream::MixEncoder::new(*rate)));
                }
                blocks.clear();
                t.read(&mut blocks);
                if let Some((_, e)) = enc.as_mut() {
                    for p in e.feed(&blocks) {
                        let line = json!({"event": "stream", "params": p});
                        let mut dead = Vec::new();
                        for (id, out) in &subs {
                            if !write_line(out, &line) {
                                dead.push(*id);
                            }
                        }
                        if !dead.is_empty() {
                            lock(&inner.stream_subs).retain(|(id, _)| !dead.contains(id));
                        }
                    }
                }
            }
            _ => {
                if let Some((t, _)) = &tap {
                    t.set_on(false);
                }
                enc = None;
            }
        }
        drop(inner);
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// How long an invite is good for (minutes).
pub const INVITE_MINUTES: f64 = 30.0;

/// The largest audio take accepted (24 h of 8 channels at 96 kHz, 24-bit).
const MAX_TAKE_BYTES: u64 = 200 << 30;

/// A file name made safe (no folders, nothing Windows refuses).
fn file_name(s: &str) -> String {
    let n: String = s.chars().map(|c| if r#"\/:*?"<>|"#.contains(c) || c.is_control() { '_' } else { c }).collect();
    let n = n.trim().trim_matches('.').to_string();
    if n.is_empty() { "take".into() } else { n }
}

/// Booth timeline seconds of booth take-clock time `rolled`.
fn timeline_of(anchor: &BoothAnchor, rolled: f64) -> f64 {
    anchor.place(rolled).0
}

/// `1284` → `1,284`.
pub fn group_digits(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A tiny HTTP client for tests and tools: one request, the status code and body.
pub fn http_request(addr: SocketAddr, method: &str, path: &str, token: Option<&str>, body: Option<&Value>) -> Result<(u16, Value), String> {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
    let body = body.map(Value::to_string).unwrap_or_default();
    let tok = token.map(|t| format!("X-Token: {t}\r\n")).unwrap_or_default();
    let req = format!("{method} {path} HTTP/1.1\r\nHost: x\r\n{tok}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len());
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut text = String::new();
    s.read_to_string(&mut text).map_err(|e| e.to_string())?;
    let code = text.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    Ok((code, serde_json::from_str(body).unwrap_or(Value::String(body.to_string()))))
}

/// Reads server-sent events from `GET path` until `until` accepts one (or `timeout`).
pub fn read_events(addr: SocketAddr, path: &str, timeout: Duration, mut until: impl FnMut(&Value) -> bool) -> Result<Value, String> {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_millis(200))).map_err(|e| e.to_string())?;
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).map_err(|e| e.to_string())?;
    let mut r = BufReader::new(s);
    let end = Instant::now() + timeout;
    let mut line = String::new();
    let mut pending: VecDeque<Value> = VecDeque::new();
    while Instant::now() < end {
        line.clear();
        match r.read_line(&mut line) {
            Ok(0) => return Err("closed".into()),
            Ok(_) => {
                if let Some(d) = line.trim_end().strip_prefix("data: ")
                    && let Ok(v) = serde_json::from_str::<Value>(d)
                {
                    pending.push_back(v);
                }
            }
            Err(_) => {}
        }
        while let Some(v) = pending.pop_front() {
            if until(&v) {
                return Ok(v);
            }
        }
    }
    Err("timed out waiting for the status".into())
}

impl TransportState {
    pub fn recording(self) -> bool {
        self == TransportState::Recording
    }
}
