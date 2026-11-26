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
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{Duration, Instant};

use crate::config::{NetConfig, PairedDevice, SavedInvite};
use crate::pairing::exchange::{self, Confirmed, Exchange, LinkKeys, Side};
use crate::pairing::{Confirm, PAIRING_VERSION, PIN_OFF_AFTER, PIN_STREAK, Pairing, PendingPair, PinEvent, PinGuard};
use crate::proto::{BoothTransport, FollowerReport, RemoteAction, RemoteStatus, RigStatus, SketchNote, TransportState};
use crate::take::{BoothAnchor, TakeEnvelope, remote_port_name};

/// Which link a connection came in by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkKind {
    /// The plain TCP listener on the local network (not encrypted): the phone's page, and PCs
    /// unless [`NetConfig::encrypted_only`].
    #[default]
    Plain,
    /// The encrypted local link (QUIC, bridged by [`crate::internet::Internet::serve_lan_to`]):
    /// PCs only; pairing by code binds the device to its key.
    Encrypted,
    /// Over the internet (QUIC, bridged): PCs only, pairing by invite only.
    Internet,
}

impl LinkKind {
    /// How the Network window and the status feed name it.
    pub fn text(self) -> &'static str {
        match self {
            LinkKind::Plain => "local link, not encrypted",
            LinkKind::Encrypted => "encrypted local link",
            LinkKind::Internet => "over the internet, encrypted",
        }
    }
}

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

/// What a PC with an older FreeDAWm (one that would send the code itself) is told when it asks
/// to pair.
pub const UPDATE_PC: &str =
    "update FreeDAWm on this computer to pair with this booth: it pairs PCs by a newer exchange in which the code is never sent";

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
    /// The link it came in by.
    kind: LinkKind,
    /// A listening PC (`hello {role: "listener"}`): hears the mix and reads the status, never
    /// records or sends takes; the booth never waits for it and forgets it when it goes.
    listener: bool,
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
    stream_subs: Mutex<Vec<StreamSub>>,
    streaming: AtomicBool,
    listening: Mutex<Option<Listening>>,
    /// The loopback listener the internet bridge connects to (only pairing by invite there).
    internet: Mutex<Option<Listening>>,
    /// The loopback listener the encrypted local link's bridge connects to.
    secure: Mutex<Option<Listening>>,
    /// One-time invites made (an id, their secret and when they expire, hub seconds).
    invites: Mutex<Vec<(String, String, f64)>>,
    /// When wrong invite secrets came lately (the limit on guessing).
    bad_invites: Mutex<Vec<f64>>,
    open: AtomicUsize,
    next_id: AtomicU64,
    warning: Mutex<Option<String>>,
    /// The booth's playhead now (seconds), for a follower's greeting.
    position: Mutex<f64>,
    /// Wakes the UI thread when a request comes in.
    waker: Mutex<Option<Waker>>,
    /// What the booth's screen tells its owner until dismissed (a device paired with the booth
    /// PIN, PIN pairing paused or turned off): an id and the text.
    notices: Mutex<Vec<(u64, String)>>,
    /// The mixer feed (`mixer.subscribe`): the last picture, recent deltas, the latest meters;
    /// `mixer_changed` wakes its pushers, `mixer_subs` counts them.
    mixer: Mutex<MixerFeed>,
    mixer_changed: Condvar,
    mixer_subs: AtomicUsize,
    /// Paired PCs connected now (after `hello`), whatever they do.
    pcs: AtomicUsize,
    /// Controls a remote's hand is on (`engine.execute {gesture}`), by gesture key: the first
    /// touch holds the control until its release plus [`HOLD_GRACE`] (or [`HOLD_LEASE`] without
    /// a move), and another remote's move of it is refused meanwhile.
    holds: Mutex<HashMap<String, Hold>>,
    /// The PCs' connections after `hello`: (connection, the device's token, its name, its line
    /// out), so the booth can end them (Disconnect, Forget) or tell them the lock changed.
    conns: Mutex<Vec<(u64, String, String, Arc<Mutex<TcpStream>>)>>,
    /// Devices sent away by Disconnect (their tokens): refused until let back in or the app
    /// restarts.
    away: Mutex<Vec<String>>,
    /// Remote mixing so far, by device name: when its last change came.
    mixing: Mutex<HashMap<String, Instant>>,
}

/// Why a change from a remote is refused while remote mixing is locked.
pub const LOCKED: &str = "remote mixing is locked on the studio (Setup › Network): you can watch and listen";

/// The methods a paired PC may still call while remote mixing is locked: they read, or end a
/// gesture (its undo step closes).
const READ_ONLY: &[&str] =
    &["session.inspect", "engine.commands", "engine.parity", "ui.inspect", "ui.menu.list", "ui.screenshot", "engine.end_gesture"];

/// Whether `method` with `params` changes nothing on the studio (allowed while locked).
fn reads_only(method: &str, params: &Value) -> bool {
    if READ_ONLY.contains(&method) {
        return true;
    }
    if method == "engine.execute" || method == "command" {
        let id = params.get("command").or_else(|| params.get("id")).and_then(Value::as_str).unwrap_or("");
        return params.get("gesture").is_none() && soundcraft_engine::cmd::find_command(id).is_some_and(|c| !c.journal && !c.undoable && !c.view);
    }
    false
}

/// A PC connected now, for Setup › Network's list.
#[derive(Debug, Clone, PartialEq)]
pub struct RemotePc {
    pub name: String,
    pub token: String,
    /// It changed the studio's mix since it connected; how long ago the last change was (s).
    pub mixing: bool,
    pub last_change: Option<f64>,
}

/// A remote's hand on a control (see `Inner::holds`).
#[derive(Debug, Clone)]
struct Hold {
    /// The connection holding it, and the PC's name (the refusal says whose hand it is).
    conn: u64,
    name: String,
    /// Still down (no `engine.end_gesture` yet), and the last move or the release.
    open: bool,
    last: Instant,
}

/// A control stays its holder's this long after the release.
pub const HOLD_GRACE: Duration = Duration::from_millis(500);
/// A hold with no move this long ends by itself (a remote that never let go): as the engine's
/// gesture lease.
pub const HOLD_LEASE: Duration = Duration::from_millis(1500);

impl Hold {
    fn alive(&self) -> bool {
        let age = self.last.elapsed();
        if self.open { age < HOLD_LEASE } else { age < HOLD_GRACE }
    }
}

/// The mixer feed's shared state (see [`Hub::publish_mixer`]). Bounded: a pusher that falls
/// further behind than [`MAX_MIXER_DELTAS`] gets a fresh snapshot instead.
#[derive(Default)]
struct MixerFeed {
    seq: u64,
    state: Option<Arc<Value>>,
    /// `(seq, line)`: the newest last.
    deltas: VecDeque<(u64, Arc<String>)>,
    meter_seq: u64,
    meters: Option<Arc<String>>,
    last_meters: Option<Value>,
}

/// Mixer deltas kept for pushers catching up.
const MAX_MIXER_DELTAS: usize = 64;
/// Meters go to a subscriber at most this often (~20 Hz).
const METERS_EVERY: Duration = Duration::from_millis(50);

/// A link hearing the mix stream.
#[derive(Clone)]
struct StreamSub {
    link: u64,
    out: Arc<Mutex<TcpStream>>,
    /// It takes the stream while the booth is stopped too (`stream.subscribe {live: true}`: a
    /// newer FreeDAWm, whose decoder knows idle packets); else talkback only then, as before.
    live: bool,
}

/// Where booth PIN pairing stands (Setup › Network says it).
#[derive(Debug, Clone, PartialEq)]
pub enum PinState {
    /// No booth PIN: a random code per request.
    Unset,
    /// Devices pair with the booth PIN.
    On,
    /// Paused after wrong PINs: seconds left, and where the last wrong one came from.
    Paused { secs_left: f64, from: String },
    /// Turned off after [`PIN_OFF_AFTER`] wrong PINs, until the owner turns it back on.
    Off { from: String },
}

/// The most notices kept.
const MAX_NOTICES: usize = 20;

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
                secure: Mutex::new(None),
                invites: Mutex::new(Vec::new()),
                bad_invites: Mutex::new(Vec::new()),
                open: AtomicUsize::new(0),
                next_id: AtomicU64::new(1),
                warning: Mutex::new(None),
                position: Mutex::new(0.0),
                waker: Mutex::new(None),
                notices: Mutex::new(Vec::new()),
                mixer: Mutex::new(MixerFeed::default()),
                mixer_changed: Condvar::new(),
                mixer_subs: AtomicUsize::new(0),
                pcs: AtomicUsize::new(0),
                holds: Mutex::new(HashMap::new()),
                conns: Mutex::new(Vec::new()),
                away: Mutex::new(Vec::new()),
                mixing: Mutex::new(HashMap::new()),
            }),
        }
        .seeded()
    }

    /// The wrong-PIN count carried over from the saved settings.
    fn seeded(self) -> Hub {
        let (wrong, from) = {
            let c = lock(&self.inner.cfg);
            (c.pin_wrong, c.pin_last_from.clone())
        };
        lock(&self.inner.pairing).set_pin_guard(PinGuard { wrong, last_from: from, ..PinGuard::default() });
        self
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

    /// Lets a device waiting with the booth PIN pair (Allow, when the booth asks before each
    /// device joins).
    pub fn allow_pairing(&self, id: &str) {
        lock(&self.inner.pairing).allow(id);
    }

    pub fn forget(&self, token: &str) {
        self.update_config(|c| c.paired.retain(|d| d.token != token));
        self.end_connections(|t| t == token);
    }

    /// Forgets every paired device (each must pair again).
    pub fn forget_all(&self) {
        self.update_config(|c| c.paired.clear());
        self.end_connections(|_| true);
    }

    /// Ends the connections of the devices whose token `which` picks (they see the link drop).
    fn end_connections(&self, which: impl Fn(&str) -> bool) {
        for (_, token, _, out) in lock(&self.inner.conns).iter() {
            if which(token) {
                let _ = lock(out).shutdown(std::net::Shutdown::Both);
            }
        }
    }

    /// Disconnect (Setup › Network): ends the device's connections and refuses it until
    /// [`Hub::let_back`] (or the app restarts). It stays paired.
    pub fn disconnect(&self, token: &str) {
        {
            let mut a = lock(&self.inner.away);
            if !a.iter().any(|t| t == token) {
                a.push(token.to_string());
            }
        }
        self.end_connections(|t| t == token);
    }

    /// Lets a device sent away by [`Hub::disconnect`] connect again.
    pub fn let_back(&self, token: &str) {
        lock(&self.inner.away).retain(|t| t != token);
    }

    /// The devices sent away now: (name, token).
    pub fn sent_away(&self) -> Vec<(String, String)> {
        let away = lock(&self.inner.away).clone();
        let cfg = lock(&self.inner.cfg);
        away.into_iter().map(|t| (cfg.device(&t).map_or_else(|| "a PC".to_string(), |d| d.name.clone()), t)).collect()
    }

    /// Lock remote mixing: paired PCs watch and listen, every change from them is refused. Each
    /// PC connected is told at once (on a thread: the UI never waits on a link).
    pub fn set_remote_lock(&self, on: bool) {
        self.update_config(|c| c.lock_remote_mixing = on);
        let outs: Vec<Arc<Mutex<TcpStream>>> = lock(&self.inner.conns).iter().map(|c| Arc::clone(&c.3)).collect();
        let _ = std::thread::Builder::new().name("freedawm-net-lock".into()).spawn(move || {
            let line = json!({"event": "remote_lock", "params": {"on": on}});
            for out in outs {
                let _ = write_line(&out, &line);
            }
        });
    }

    pub fn remote_lock(&self) -> bool {
        lock(&self.inner.cfg).lock_remote_mixing
    }

    /// The paired PCs connected now (each once), and whether each is mixing.
    pub fn remote_pcs(&self) -> Vec<RemotePc> {
        let mixing = lock(&self.inner.mixing).clone();
        let mut out: Vec<RemotePc> = Vec::new();
        for (_, token, name, _) in lock(&self.inner.conns).iter() {
            if out.iter().any(|r| r.token == *token) {
                continue;
            }
            let last = mixing.get(name).map(|t| t.elapsed().as_secs_f64());
            out.push(RemotePc { name: name.clone(), token: token.clone(), mixing: last.is_some(), last_change: last });
        }
        out
    }

    /// A change from `name` reached the app: it is mixing (a notice the first time).
    fn note_mixing(&self, name: &str) {
        let first = lock(&self.inner.mixing).insert(name.to_string(), Instant::now()).is_none();
        if first {
            self.notice(format!("{name} started mixing remotely"));
        }
    }

    /// Sets the booth PIN (spaces around it dropped; at least
    /// [`crate::pairing::PIN_MIN_LEN`] characters, else why not in plain words). A new PIN starts
    /// the wrong-PIN count again and turns PIN pairing back on. Devices already paired stay
    /// paired: they hold their own tokens.
    pub fn set_booth_pin(&self, pin: &str) -> Result<(), String> {
        let pin = crate::pairing::check_pin(pin)?;
        self.update_config(|c| {
            c.booth_pin = pin;
            c.pin_wrong = 0;
            c.pin_last_from.clear();
        });
        self.reset_pin_guard();
        Ok(())
    }

    /// No booth PIN: a random code per request, as before. Devices already paired stay paired.
    pub fn clear_booth_pin(&self) {
        self.update_config(|c| c.booth_pin.clear());
        self.reset_pin_guard();
    }

    /// Ask before each device joins with the booth PIN.
    pub fn set_pin_ask(&self, ask: bool) {
        self.update_config(|c| c.pin_ask = ask);
    }

    /// The owner resumes a paused PIN pairing, or turns one turned off back on: the wrong-PIN
    /// count starts again.
    pub fn reset_pin_guard(&self) {
        lock(&self.inner.pairing).set_pin_guard(PinGuard::default());
        let saved = {
            let c = lock(&self.inner.cfg);
            c.pin_wrong != 0 || !c.pin_last_from.is_empty()
        };
        if saved {
            self.update_config(|c| {
                c.pin_wrong = 0;
                c.pin_last_from.clear();
            });
        }
    }

    /// Where booth PIN pairing stands.
    pub fn pin_state(&self) -> PinState {
        if lock(&self.inner.cfg).booth_pin.is_empty() {
            return PinState::Unset;
        }
        let now = self.now_s();
        let p = lock(&self.inner.pairing);
        let g = p.pin_guard();
        if g.is_off() {
            PinState::Off { from: g.last_from.clone() }
        } else if let Some(secs_left) = g.paused(now) {
            PinState::Paused { secs_left, from: g.last_from.clone() }
        } else {
            PinState::On
        }
    }

    /// For tests: a pause of PIN pairing ends `secs` sooner.
    pub fn age_pin_pause(&self, secs: f64) {
        let mut p = lock(&self.inner.pairing);
        let mut g = p.pin_guard().clone();
        g.paused_until -= secs;
        p.set_pin_guard(g);
    }

    /// What the booth's screen tells its owner until dismissed, oldest first.
    pub fn notices(&self) -> Vec<(u64, String)> {
        lock(&self.inner.notices).clone()
    }

    pub fn dismiss_notice(&self, id: u64) {
        lock(&self.inner.notices).retain(|(i, _)| *i != id);
    }

    /// Tells the booth's owner `text` (on screen until dismissed).
    pub fn notice(&self, text: String) {
        log::info!("network: {text}");
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        {
            let mut n = lock(&self.inner.notices);
            n.push((id, text));
            if n.len() > MAX_NOTICES {
                n.remove(0);
            }
        }
        if let Some(w) = lock(&self.inner.waker).as_ref() {
            w();
        }
    }

    /// Hands the booth PIN as set now to the pairing state (none while unset).
    fn sync_pin(&self) {
        let (pin, ask) = {
            let c = lock(&self.inner.cfg);
            (zeroize::Zeroizing::new(c.booth_pin.clone()), c.pin_ask)
        };
        lock(&self.inner.pairing).set_pin(if pin.is_empty() { None } else { Some(pin.as_str()) }, ask);
    }

    /// After a pairing call: the booth PIN's notices, and its wrong count kept in the settings.
    fn after_pin(&self) {
        let (events, guard) = {
            let mut p = lock(&self.inner.pairing);
            (p.take_pin_events(), p.pin_guard().clone())
        };
        for e in events {
            match e {
                PinEvent::Paired { name, .. } => self.notice(format!("{name} paired with the booth PIN")),
                PinEvent::Paused { from, secs } => {
                    self.notice(format!("PIN pairing paused for {} minutes: {PIN_STREAK} wrong tries from {from}", (secs / 60.0).round()))
                }
                PinEvent::TurnedOff { from } => self.notice(format!(
                    "PIN pairing turned off after {PIN_OFF_AFTER} wrong tries (the last from {from}): turn it back on in Setup › Network"
                )),
            }
        }
        let saved = {
            let c = lock(&self.inner.cfg);
            (c.pin_wrong, c.pin_last_from.clone())
        };
        if saved != (guard.wrong, guard.last_from.clone()) {
            self.update_config(|c| {
                c.pin_wrong = guard.wrong;
                c.pin_last_from = guard.last_from;
            });
        }
    }

    /// Why a try on PIN request `id` is not made: it waits for Allow on the booth's screen.
    fn pin_refusal(&self, id: &str) -> Option<String> {
        let p = lock(&self.inner.pairing);
        (p.is_pin_request(id) && p.waiting_for_allow(id))
            .then(|| "the booth asks before each device joins: someone there must press Allow, then type the PIN again".to_string())
    }

    /// What a request that is gone is told: PIN pairing paused or off says so.
    fn gone(&self) -> String {
        match self.pin_state() {
            PinState::Paused { .. } | PinState::Off { .. } => {
                "PIN pairing is paused on the booth after wrong tries: ask again (the booth then shows a code on its screen), or try later".into()
            }
            _ => "that pairing request has expired; ask again".into(),
        }
    }

    fn pair_request(&self, name: &str, kind: &str, from: &str) -> Result<Value, String> {
        let now = self.now_s();
        self.sync_pin();
        let r = lock(&self.inner.pairing)
            .request_from(name, kind, from, now, random_u64)
            .ok_or("too many pairing requests; try again in a few minutes")?;
        if r.pin {
            log::info!("pairing requested by {} ({}): waiting for the booth PIN", r.name, r.kind);
            return Ok(json!({"request": r.id, "pc": self.pc_name(), "pairing": PAIRING_VERSION, "pin": true, "ask": !r.allowed}));
        }
        log::info!("pairing requested by {} ({}): code shown on screen", r.name, r.kind);
        Ok(json!({"request": r.id, "pc": self.pc_name(), "pairing": PAIRING_VERSION}))
    }

    /// A browser typed the code or the booth PIN (the phone's page; a PC's is never sent).
    fn pair_confirm(&self, id: &str, code: &str) -> Result<Value, (String, u8)> {
        let now = self.now_s();
        self.sync_pin();
        if let Some(e) = self.pin_refusal(id) {
            return Err((e, crate::pairing::MAX_WRONG));
        }
        let got = lock(&self.inner.pairing).confirm(id, code, now);
        self.after_pin();
        match got {
            Confirm::Paired { name, kind } => Ok(self.paired(name, kind, None)),
            Confirm::Wrong { left } => Err((format!("wrong code or PIN ({left} tries left)"), left)),
            Confirm::Unknown => Err((self.gone(), 0)),
        }
    }

    /// Remembers a device just paired and answers with its token. `peer`: the PC's key when it
    /// paired over the encrypted local link (it is then bound to that key and that link).
    fn paired(&self, name: String, kind: String, peer: Option<&str>) -> Value {
        let token = new_token();
        let key = peer.unwrap_or_default().to_string();
        let dev = PairedDevice { name: name.clone(), kind, token: token.clone(), follows: false, key };
        self.update_config(|c| c.paired.push(dev));
        json!({"token": token, "name": name, "pc": self.pc_name()})
    }

    /// `pair.exchange {request, msg}`: a PC starts the key exchange for its request (see
    /// [`crate::pairing::exchange`]), spending one of its tries. `keys`: the two link keys as this
    /// connection's TLS session proved them (`LinkKeys::PLAIN` on the plain link). Returns the
    /// request, this side's confirmation state and the answer (the booth's message and
    /// confirmation). Every failure after the try was spent ends the exchange as failed.
    fn pair_exchange(&self, params: &Value, keys: Option<LinkKeys>) -> Result<(String, Confirmed, Value), String> {
        let keys = keys.ok_or("the encrypted link did not say which keys it proved")?;
        let id = params.get("request").and_then(Value::as_str).filter(|s| s.len() <= 64).ok_or("no pairing request named")?;
        self.sync_pin();
        if let Some(e) = self.pin_refusal(id) {
            return Err(e);
        }
        let code = lock(&self.inner.pairing).begin_exchange(id, self.now_s());
        self.after_pin();
        let code = code.ok_or_else(|| match self.pin_state() {
            PinState::Paused { .. } | PinState::Off { .. } => self.gone(),
            _ => "that pairing request has expired or has no tries left; ask again".to_string(),
        })?;
        let theirs = params
            .get("msg")
            .and_then(Value::as_str)
            .filter(|s| s.len() <= 4 * exchange::MSG_LEN)
            .and_then(crate::audio::b64_decode)
            .filter(|m| m.len() == exchange::MSG_LEN);
        let confirmed = theirs.ok_or(exchange::Damaged).and_then(|theirs| {
            let (ex, mine) = Exchange::start(Side::Booth, &code, keys);
            ex.finish(&theirs).map(|c| (c, mine))
        });
        drop(code);
        match confirmed {
            Ok((c, mine)) => {
                let answer = json!({"msg": crate::audio::b64_encode(&mine), "confirm": crate::audio::b64_encode(&c.tag())});
                Ok((id.to_string(), c, answer))
            }
            Err(e) => {
                self.pair_exchange_failed(id);
                Err(e.to_string())
            }
        }
    }

    /// `pair.finish {confirm}`: the PC's confirmation for the exchange `id` this connection began.
    /// Pairs when it matches (compared in constant time); otherwise the try stays spent.
    fn pair_finish(&self, id: &str, c: &Confirmed, params: &Value, peer: Option<&str>) -> Result<Value, String> {
        let tag = params.get("confirm").and_then(Value::as_str).filter(|s| s.len() <= 4 * exchange::TAG_LEN).and_then(crate::audio::b64_decode);
        let ok = tag.is_some_and(|t| c.check(&t));
        let got = lock(&self.inner.pairing).finish_exchange(id, ok, self.now_s());
        self.after_pin();
        match got {
            Confirm::Paired { name, kind } => {
                log::info!("network: {name} paired; its code proved both PCs' keys");
                Ok(self.paired(name, kind, peer))
            }
            Confirm::Wrong { left } => {
                log::info!("network: a PC's pairing exchange failed (a wrong code, or another computer in between)");
                Err(format!("wrong code or PIN, or another computer is in between ({left} tries left)"))
            }
            Confirm::Unknown => Err(self.gone()),
        }
    }

    /// An exchange begun for `id` ended without a match (its try stays spent).
    fn pair_exchange_failed(&self, id: &str) {
        let now = self.now_s();
        let _ = lock(&self.inner.pairing).finish_exchange(id, false, now);
        self.after_pin();
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
            if l.listener {
                if l.connected {
                    let mut parts: Vec<String> = Vec::new();
                    let last = lock(&self.inner.mixing).get(&l.name).map(|t| t.elapsed().as_secs_f64());
                    if let Some(s) = last {
                        parts.push(format!("last change {}", ago(s)));
                    }
                    if l.streaming {
                        parts.push("hears the mix".into());
                    }
                    parts.push(l.kind.text().into());
                    let state = if last.is_some() { "mixing" } else { "listening" };
                    out.push(RemoteStatus { name: l.name.clone(), state: state.into(), detail: parts.join(" · "), ..RemoteStatus::default() });
                }
                continue;
            }
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
            if l.connected {
                parts.push(l.kind.text().into());
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

    /// A link wants the mix (or not; `live`: while the booth is stopped too): the stream thread
    /// starts with the first.
    fn subscribe(&self, link: u64, out: &Arc<Mutex<TcpStream>>, on: bool, live: bool) {
        {
            let mut subs = lock(&self.inner.stream_subs);
            subs.retain(|s| s.link != link);
            if on {
                subs.push(StreamSub { link, out: Arc::clone(out), live });
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
            .spawn(move || hub.accept_loop(listener, stop, LinkKind::Plain))
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
            .spawn(move || hub.accept_loop(listener, stop, LinkKind::Internet))
            .map_err(|e| e.to_string())?;
        Ok(port)
    }

    /// The listener the encrypted local link's bridge connects to
    /// ([`crate::internet::Internet::serve_lan_to`]): loopback only, any free port; JSON lines
    /// only, each connection's first line the caller's key. Kept until [`Hub::stop_secure`];
    /// returns its port.
    pub fn listen_secure(&self) -> Result<u16, String> {
        if let Some(l) = lock(&self.inner.secure).as_ref() {
            return Ok(l.port);
        }
        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("cannot listen for the encrypted link: {e}"))?;
        let port = listener.local_addr().map(|a| a.port()).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        *lock(&self.inner.secure) = Some(Listening { port, stop: Arc::clone(&stop) });
        let hub = self.clone();
        std::thread::Builder::new()
            .name("freedawm-net-accept-secure".into())
            .spawn(move || hub.accept_loop(listener, stop, LinkKind::Encrypted))
            .map_err(|e| e.to_string())?;
        Ok(port)
    }

    /// The encrypted link's listener port, while there is one.
    pub fn secure_port(&self) -> Option<u16> {
        lock(&self.inner.secure).as_ref().map(|l| l.port)
    }

    pub fn stop_secure(&self) {
        if let Some(l) = lock(&self.inner.secure).take() {
            l.stop.store(true, Ordering::SeqCst);
            self.inner.changed.notify_all();
            self.inner.transport_changed.notify_all();
        }
    }

    /// The remote recorders connected now and the link each came by (the Network window).
    pub fn links(&self) -> Vec<(String, LinkKind)> {
        lock(&self.inner.links).iter().filter(|l| l.connected).map(|l| (l.name.clone(), l.kind)).collect()
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
    /// The secret is a pairing token's 128 random bits from the OS.
    pub fn new_invite_secret(&self) -> String {
        let secret = new_token();
        let until = self.now_s() + INVITE_MINUTES * 60.0;
        let mut inv = lock(&self.inner.invites);
        let now = self.now_s();
        inv.retain(|(_, _, t)| *t > now);
        inv.push((format!("{:08x}", random_u64() & 0xffff_ffff), secret.clone(), until));
        secret
    }

    /// A reusable invite secret (as strong as a one-time one): good for `secs` and
    /// `max_joins` devices (1 to [`MAX_INVITE_JOINS`]), kept in the settings as its SHA-256 only
    /// so it outlives a restart. Devices it paired stay paired once it is revoked or ends.
    pub fn new_reusable_invite(&self, secs: u64, max_joins: u32) -> String {
        let secret = new_token();
        let saved = SavedInvite {
            id: format!("{:08x}", random_u64() & 0xffff_ffff),
            hash: secret_hash(&secret),
            until: unix_now().saturating_add(secs),
            max_joins: max_joins.clamp(1, MAX_INVITE_JOINS),
            joins: 0,
        };
        self.update_config(|c| {
            let now = unix_now();
            c.invites.retain(|i| i.until > now && i.joins < i.max_joins);
            c.invites.push(saved);
        });
        secret
    }

    /// Invites waiting (not used, not expired): seconds each has left.
    pub fn open_invites(&self) -> Vec<f64> {
        self.invites().iter().map(|i| i.secs_left).collect()
    }

    /// Every invite still open, one-time ones first, for Setup › Network's list.
    pub fn invites(&self) -> Vec<OpenInvite> {
        let now = self.now_s();
        let mut v: Vec<OpenInvite> = lock(&self.inner.invites)
            .iter()
            .filter(|(_, _, t)| *t > now)
            .map(|(id, _, t)| OpenInvite { id: id.clone(), reusable: false, secs_left: t - now, joins: 0, max_joins: 1 })
            .collect();
        let unix = unix_now();
        v.extend(lock(&self.inner.cfg).invites.iter().filter(|i| i.until > unix && i.joins < i.max_joins).map(|i| OpenInvite {
            id: i.id.clone(),
            reusable: true,
            secs_left: (i.until - unix) as f64,
            joins: i.joins,
            max_joins: i.max_joins,
        }));
        v
    }

    /// Ends the invite `id` now (devices it paired stay paired).
    pub fn revoke_invite(&self, id: &str) {
        lock(&self.inner.invites).retain(|(i, _, _)| i != id);
        if lock(&self.inner.cfg).invites.iter().any(|i| i.id == id) {
            self.update_config(|c| c.invites.retain(|i| i.id != id));
        }
    }

    /// For tests: invites expire `secs` from now.
    pub fn age_invites(&self, secs: f64) {
        for (_, _, t) in lock(&self.inner.invites).iter_mut() {
            *t -= secs;
        }
        let s = secs.max(0.0) as u64;
        self.update_config(|c| {
            for i in &mut c.invites {
                i.until = i.until.saturating_sub(s);
            }
        });
    }

    /// For tests: wrong invite secrets are forgotten.
    pub fn forget_bad_invites(&self) {
        lock(&self.inner.bad_invites).clear();
    }

    /// `pair.invite {secret, name, token?}`: a one-time invite's secret, once, before it expires,
    /// pairs `name`; a reusable one's, until it ends or its joins are used. `token`: the device's
    /// own from an earlier join of this booth, recognised (its pairing kept, no join used) while
    /// the invite is still good. Wrong secrets are limited across the listener.
    fn pair_invite(&self, secret: &str, name: &str, token: Option<&str>) -> Result<Value, String> {
        const REFUSED: &str = "that invite has expired or was already used: ask for a new one";
        let now = self.now_s();
        {
            let mut bad = lock(&self.inner.bad_invites);
            bad.retain(|t| now - t < BAD_INVITE_WINDOW_S);
            if bad.len() >= MAX_BAD_INVITES {
                return Err("too many wrong invites lately: try again in a few minutes".into());
            }
        }
        let hash = secret_hash(secret);
        let unix = unix_now();
        // Which invite: a one-time one (index) or a reusable one (id).
        let one_time = lock(&self.inner.invites).iter().position(|(_, s, t)| !secret.is_empty() && same_secret(s, secret) && *t > now);
        let reusable = if one_time.is_some() || secret.is_empty() {
            None
        } else {
            lock(&self.inner.cfg)
                .invites
                .iter()
                .find(|i| same_secret(&i.hash, &hash) && i.until > unix && i.joins < i.max_joins)
                .map(|i| i.id.clone())
        };
        if one_time.is_none() && reusable.is_none() {
            lock(&self.inner.bad_invites).push(now);
            return Err(REFUSED.into());
        }
        // A device that joined before (its token, from an invite): recognised, nothing used.
        if let Some(dev) = token.and_then(|t| self.device(t)).filter(|d| d.kind == "pc" && d.key.is_empty()) {
            log::info!("network: {} rejoined with an invite it had used", dev.name);
            return Ok(json!({"token": dev.token, "name": dev.name, "pc": self.pc_name(), "again": true}));
        }
        if let Some(i) = one_time {
            let mut inv = lock(&self.inner.invites);
            if inv.get(i).is_some_and(|(_, s, _)| same_secret(s, secret)) {
                inv.remove(i);
            } else {
                return Err(REFUSED.into());
            }
        }
        let name = crate::pairing::clean_name(name);
        let token = new_token();
        let dev = PairedDevice { name: name.clone(), kind: "pc".into(), token: token.clone(), follows: false, key: String::new() };
        let mut used = true;
        self.update_config(|c| {
            if let Some(id) = &reusable {
                match c.invites.iter_mut().find(|i| &i.id == id && i.joins < i.max_joins) {
                    Some(i) => i.joins = i.joins.saturating_add(1),
                    None => {
                        used = false;
                        return;
                    }
                }
            }
            c.paired.push(dev);
        });
        if !used {
            return Err(REFUSED.into());
        }
        self.notice(if reusable.is_some() { format!("{name} joined with the reusable invite") } else { format!("{name} joined with an invite") });
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

    fn accept_loop(&self, listener: TcpListener, stop: Arc<AtomicBool>, kind: LinkKind) {
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
                        hub.serve(stream, addr, &stop, kind);
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

    fn serve(&self, stream: TcpStream, addr: SocketAddr, stop: &Arc<AtomicBool>, kind: LinkKind) {
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
            self.serve_lines(reader, stream, addr, stop, kind);
        } else if kind != LinkKind::Plain {
            // The service remote page is on the plain listener only.
        } else {
            self.serve_http(reader, stream, addr, stop);
        }
    }

    // ---- HTTP: the service remote page ----

    fn serve_http(&self, mut reader: BufReader<TcpStream>, mut out: TcpStream, addr: SocketAddr, stop: &Arc<AtomicBool>) {
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
                match self.pair_request(name, "browser", &from_addr(addr)) {
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

    /// Connection `conn` (PC `name`) moves the control of gesture `key`: taken, or refused with
    /// the holder's name while another connection's hand is on it.
    fn hold(&self, conn: u64, key: &str, name: &str) -> Result<(), String> {
        let mut h = lock(&self.inner.holds);
        h.retain(|_, x| x.alive());
        if let Some(x) = h.get(key).filter(|x| x.conn != conn) {
            return Err(x.name.clone());
        }
        h.insert(key.to_string(), Hold { conn, name: name.to_string(), open: true, last: Instant::now() });
        Ok(())
    }

    /// Connection `conn` let go of `key`: still its own for [`HOLD_GRACE`].
    fn release_hold(&self, conn: u64, key: &str) {
        if let Some(x) = lock(&self.inner.holds).get_mut(key).filter(|x| x.conn == conn) {
            x.open = false;
            x.last = Instant::now();
        }
    }

    fn drop_holds(&self, conn: u64) {
        lock(&self.inner.holds).retain(|_, x| x.conn != conn);
    }

    /// The controls held by a remote's hand now: (gesture key, PC name).
    pub fn holds(&self) -> Vec<(String, String)> {
        let mut h = lock(&self.inner.holds);
        h.retain(|_, x| x.alive());
        h.iter().map(|(k, x)| (k.clone(), x.name.clone())).collect()
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

    fn serve_lines(&self, mut reader: BufReader<TcpStream>, out: TcpStream, addr: SocketAddr, stop: &Arc<AtomicBool>, kind: LinkKind) {
        let internet = kind == LinkKind::Internet;
        // The caller's key on the encrypted link (its bridge's first line), and this PC's own as
        // that connection's TLS session proved them.
        let mut peer: Option<String> = None;
        let mut me: Option<String> = None;
        // A PC's pairing exchange under way on this connection: its request and this side's
        // confirmation state. Anything but `pair.finish` next ends it as failed.
        let mut exchange: Option<(String, Confirmed)> = None;
        let out = Arc::new(Mutex::new(out));
        let mut buf = Vec::new();
        let mut who: Option<PairedDevice> = None;
        let mut link: Option<u64> = None;
        let mut pusher: Option<Arc<AtomicBool>> = None;
        // This connection is a listening PC's (not a recorder's).
        let mut listening = false;
        // The status and mixer feeds pushed on this connection, while on.
        let mut status_push: Option<Arc<AtomicBool>> = None;
        let mut mixer_push: Option<Arc<AtomicBool>> = None;
        let mut counted = false;
        // This connection (its holds), and the gestures it has open now.
        let conn = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let mut gestures: Vec<String> = Vec::new();
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
                    if let Some((id, _)) = exchange.take() {
                        self.pair_exchange_failed(&id);
                    }
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
            let plain_refused = kind == LinkKind::Plain && lock(&self.inner.cfg).encrypted_only;
            // An exchange is answered by `pair.finish` at once, or it failed (its try is spent).
            if method != "pair.finish"
                && let Some((id, _)) = exchange.take()
            {
                self.pair_exchange_failed(&id);
            }
            // This line's answer ends the connection (a failed pairing exchange).
            let mut close = false;
            let reply: Option<Value> = match method.as_str() {
                "link.peer" => {
                    let key = params.get("key").and_then(Value::as_str).filter(|k| crate::internet::key_from_hex(k).is_some());
                    let own = params.get("me").and_then(Value::as_str).filter(|k| crate::internet::key_from_hex(k).is_some());
                    match (key, own) {
                        (Some(k), Some(m)) if kind == LinkKind::Encrypted && peer.is_none() && who.is_none() => {
                            peer = Some(k.to_string());
                            me = Some(m.to_string());
                            None
                        }
                        _ => Some(json!({"ok": false, "error": "not on this link"})),
                    }
                }
                _ if kind == LinkKind::Encrypted && peer.is_none() => {
                    Some(json!({"ok": false, "error": "the encrypted link did not say who is calling"}))
                }
                "pair.request" | "pair.confirm" | "pair.exchange" | "pair.finish" | "pair.invite" | "hello" if plain_refused => Some(
                    json!({"ok": false, "error": "this booth takes PCs over the encrypted local link only (Setup › Network on the booth)", "pair": true}),
                ),
                "pair.request" | "pair.confirm" | "pair.exchange" | "pair.finish" if internet => {
                    Some(json!({"ok": false, "error": "over the internet, pair with an invite from the booth", "pair": true}))
                }
                "pair.invite" => Some(answer(self.pair_invite(
                    params.get("secret").and_then(Value::as_str).unwrap_or(""),
                    params.get("name").and_then(Value::as_str).unwrap_or("PC"),
                    params.get("token").and_then(Value::as_str),
                ))),
                // A PC with an older FreeDAWm says no version (it would send the code itself):
                // told to update before any code is shown.
                "pair.request" if params.get("pairing").and_then(Value::as_u64).is_none_or(|v| v < PAIRING_VERSION) => {
                    Some(json!({"ok": false, "error": UPDATE_PC, "pair": true, "update": true}))
                }
                "pair.request" => Some(answer(self.pair_request(params.get("name").and_then(Value::as_str).unwrap_or("PC"), "pc", &from_addr(addr)))),
                // The code in the clear (an older FreeDAWm): never compared, never accepted.
                "pair.confirm" => Some(json!({"ok": false, "error": UPDATE_PC, "pair": true, "update": true})),
                "pair.exchange" => {
                    let keys = match kind {
                        LinkKind::Encrypted => peer.as_deref().zip(me.as_deref()).and_then(|(p, m)| LinkKeys::from_hex(m, p)),
                        _ => Some(LinkKeys::PLAIN),
                    };
                    match self.pair_exchange(&params, keys) {
                        Ok((id, c, result)) => {
                            exchange = Some((id, c));
                            Some(json!({"ok": true, "result": result}))
                        }
                        Err(e) => {
                            close = true;
                            Some(json!({"ok": false, "error": e, "pair": true}))
                        }
                    }
                }
                "pair.finish" => {
                    close = true;
                    match exchange.take() {
                        Some((id, c)) => {
                            let r = self.pair_finish(&id, &c, &params, if kind == LinkKind::Encrypted { peer.as_deref() } else { None });
                            close = r.is_err();
                            Some(answer(r))
                        }
                        None => Some(json!({"ok": false, "error": "no pairing exchange on this connection", "pair": true})),
                    }
                }
                "hello" => {
                    let token = params.get("token").and_then(Value::as_str).unwrap_or("");
                    match self.device(token) {
                        None => Some(json!({"ok": false, "error": "not paired", "pair": true})),
                        // Paired over the encrypted link: only over it, and only with its key.
                        Some(dev) if !dev.key.is_empty() && (kind != LinkKind::Encrypted || peer.as_deref() != Some(dev.key.as_str())) => Some(
                            json!({"ok": false, "error": "this PC was paired over the encrypted local link: connect over it, with the same key", "pair": true}),
                        ),
                        Some(dev) if lock(&self.inner.away).contains(&dev.token) => Some(
                            json!({"ok": false, "error": "the studio disconnected this PC: it can let it back in from Setup › Network", "away": true}),
                        ),
                        Some(dev) => {
                            if who.is_none() {
                                lock(&self.inner.conns).push((conn, dev.token.clone(), dev.name.clone(), Arc::clone(&out)));
                            }
                            let role = params.get("role").and_then(Value::as_str);
                            let follower = role == Some("follower");
                            listening = role == Some("listener") && link.is_none();
                            if !counted {
                                counted = true;
                                self.inner.pcs.fetch_add(1, Ordering::SeqCst);
                            }
                            if follower || listening {
                                let report: FollowerReport = if follower {
                                    params.get("report").cloned().and_then(|r| serde_json::from_value(r).ok()).unwrap_or_default()
                                } else {
                                    FollowerReport::default()
                                };
                                let l = self.open_link(&dev, report, kind, listening);
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
                            let what = if follower {
                                " as a remote recorder"
                            } else if listening {
                                " to listen"
                            } else {
                                ""
                            };
                            log::info!("network: {} connected from {addr}{what}", dev.name);
                            let r = json!({"ok": true, "result": {"pc": self.pc_name(), "name": dev.name, "transport": self.transport(), "locked": self.remote_lock()}});
                            who = Some(dev);
                            Some(r)
                        }
                    }
                }
                _ if who.is_none() => Some(json!({"ok": false, "error": "not paired: send pair.request / pair.confirm, then hello", "pair": true})),
                // A round trip that never reaches the app (the Remote Mix window's link quality).
                "net.ping" => Some(json!({"ok": true, "result": {}})),
                "sync.ping" => {
                    let now = self.clock_now();
                    match (arrived, now) {
                        (Some(b1), Some(b2)) => Some(json!({"ok": true, "result": {"b1": b1, "b2": b2}})),
                        _ => Some(json!({"ok": false, "error": "the booth is stopped"})),
                    }
                }
                "follower.report" | "feed" | "feed.audio" | "take.offer" | "take.chunk" | "take.transfer" if listening => {
                    Some(json!({"ok": false, "error": "a listening PC does not record: connect as a remote recorder to send takes"}))
                }
                "status.subscribe" => {
                    let on = params.get("on").and_then(Value::as_bool).unwrap_or(true);
                    if let Some(f) = status_push.take() {
                        f.store(true, Ordering::SeqCst);
                        self.inner.changed.notify_all();
                    }
                    if on {
                        let flag = Arc::new(AtomicBool::new(false));
                        status_push = Some(Arc::clone(&flag));
                        let (hub, out, stop) = (self.clone(), Arc::clone(&out), Arc::clone(stop));
                        let _ = std::thread::Builder::new().name("freedawm-net-status".into()).spawn(move || hub.push_status(&out, &flag, &stop));
                    }
                    Some(json!({"ok": true, "result": {"on": on}}))
                }
                "mixer.subscribe" => {
                    let on = params.get("on").and_then(Value::as_bool).unwrap_or(true);
                    let meters = params.get("meters").and_then(Value::as_bool).unwrap_or(true);
                    if let Some(f) = mixer_push.take() {
                        f.store(true, Ordering::SeqCst);
                        self.inner.mixer_changed.notify_all();
                    }
                    if on {
                        let flag = Arc::new(AtomicBool::new(false));
                        mixer_push = Some(Arc::clone(&flag));
                        let (hub, out, stop) = (self.clone(), Arc::clone(&out), Arc::clone(stop));
                        let _ =
                            std::thread::Builder::new().name("freedawm-net-mixer".into()).spawn(move || hub.push_mixer(&out, &flag, &stop, meters));
                    }
                    Some(json!({"ok": true, "result": {"on": on, "meters": meters}}))
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
                    let live = params.get("live").and_then(Value::as_bool).unwrap_or(false);
                    match link {
                        Some(l) => {
                            self.subscribe(l, &out, on, live);
                            Some(json!({"ok": true, "result": {"on": on, "live": live}}))
                        }
                        None => Some(json!({"ok": false, "error": "only a remote recorder or a listening PC hears the mix"})),
                    }
                }
                "take.offer" => Some(answer(who.as_ref().map_or(Err("not paired".into()), |d| self.take_offer(&d.name, &params)))),
                "take.chunk" => Some(answer(self.take_chunk(&params))),
                "take.transfer" => match serde_json::from_value::<TakeEnvelope>(params) {
                    Ok(t) => Some(answer(self.receive_take(t))),
                    Err(e) => Some(json!({"ok": false, "error": format!("bad take: {e}")})),
                },
                _ if self.remote_lock() && !reads_only(&method, &params) => Some(json!({"ok": false, "error": LOCKED, "locked": true})),
                _ => {
                    let gesture = params.get("gesture").and_then(Value::as_str).map(str::to_string);
                    let name = who.as_ref().map_or_else(String::new, |d| d.name.clone());
                    if !reads_only(&method, &params) && method != "engine.end_gesture" {
                        self.note_mixing(&name);
                    }
                    let held = match (&gesture, method.as_str()) {
                        (Some(g), "engine.execute" | "command") => self.hold(conn, g, &name).err(),
                        (Some(g), "engine.end_gesture") => {
                            self.release_hold(conn, g);
                            gestures.retain(|x| x != g);
                            None
                        }
                        _ => None,
                    };
                    if let Some(holder) = held {
                        Some(json!({"ok": false, "error": format!("{holder} is moving that control"), "held_by": holder}))
                    } else {
                        if let Some(g) = gesture.filter(|_| method == "engine.execute" || method == "command")
                            && !gestures.contains(&g)
                        {
                            gestures.push(g);
                        }
                        let (tx, rx) = channel();
                        if !self.send(HubRequest::Control { method, params, reply: tx }) {
                            break;
                        }
                        Some(rx.recv_timeout(Duration::from_secs(90)).unwrap_or_else(|_| json!({"ok": false, "error": "timeout"})))
                    }
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
            if close {
                break;
            }
        }
        if let Some((id, _)) = exchange.take() {
            self.pair_exchange_failed(&id);
        }
        // A remote gone mid-drag: its hands come off, and the studio closes each open undo step
        // now (not when the lease runs out); the mix stays as last set.
        for g in gestures {
            self.release_hold(conn, &g);
            let (tx, _) = channel();
            self.send(HubRequest::Control { method: "engine.end_gesture".into(), params: json!({"gesture": g}), reply: tx });
        }
        self.drop_holds(conn);
        {
            let mut conns = lock(&self.inner.conns);
            let name = conns.iter().find(|c| c.0 == conn).map(|c| c.2.clone());
            conns.retain(|c| c.0 != conn);
            // Mixing ends with its last connection.
            if let Some(n) = name
                && !conns.iter().any(|c| c.2 == n)
            {
                lock(&self.inner.mixing).remove(&n);
            }
        }
        if let Some(p) = pusher {
            p.store(true, Ordering::SeqCst);
            self.inner.transport_changed.notify_all();
        }
        if let Some(p) = status_push {
            p.store(true, Ordering::SeqCst);
            self.inner.changed.notify_all();
        }
        if let Some(p) = mixer_push {
            p.store(true, Ordering::SeqCst);
            self.inner.mixer_changed.notify_all();
        }
        if counted {
            self.inner.pcs.fetch_sub(1, Ordering::SeqCst);
        }
        if let Some(l) = link {
            self.subscribe(l, &out, false, false);
            self.close_link(l);
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

    /// Pushes the status to one paired PC (`status.subscribe`): on every change, at most every
    /// 66 ms, and every 10 s when nothing changed, as `{"event": "status", "params": <status>}`.
    fn push_status(&self, out: &Arc<Mutex<TcpStream>>, done: &AtomicBool, stop: &AtomicBool) {
        let mut seen = u64::MAX;
        let mut last_push = Instant::now() - STATUS_EVERY;
        loop {
            let seq = {
                let s = lock(&self.inner.status);
                let (s, _) = self
                    .inner
                    .changed
                    .wait_timeout_while(s, Duration::from_secs(10), |s| s.0 == seen && !done.load(Ordering::SeqCst) && !stop.load(Ordering::SeqCst))
                    .unwrap_or_else(PoisonError::into_inner);
                s.0
            };
            if done.load(Ordering::SeqCst) || stop.load(Ordering::SeqCst) {
                break;
            }
            if seq != seen {
                let wait = STATUS_EVERY.saturating_sub(last_push.elapsed());
                if !wait.is_zero() {
                    std::thread::sleep(wait);
                }
            }
            seen = lock(&self.inner.status).0;
            last_push = Instant::now();
            let latest = Arc::clone(&lock(&self.inner.status).2);
            if !write_raw(out, &format!("{{\"event\":\"status\",\"params\":{latest}}}")) {
                break;
            }
        }
    }

    /// The mixer as the UI sees it now ([`soundcraft_engine::surface::feed::mixer_state`]): the
    /// first becomes the snapshot, each later one that differs a delta for the subscribers. Cheap
    /// and never waits on a subscriber (the UI thread calls it).
    pub fn publish_mixer(&self, state: Value) {
        let mut m = lock(&self.inner.mixer);
        let delta = match &m.state {
            None => None,
            Some(old) => match soundcraft_engine::surface::feed::mixer_delta(old, &state) {
                Some(d) => Some(d),
                None => return,
            },
        };
        m.seq = m.seq.wrapping_add(1);
        let seq = m.seq;
        if let Some(mut d) = delta {
            if let Some(o) = d.as_object_mut() {
                o.insert("seq".into(), json!(seq));
            }
            let line = Arc::new(json!({"event": "mixer.delta", "params": d}).to_string());
            m.deltas.push_back((seq, line));
            while m.deltas.len() > MAX_MIXER_DELTAS {
                m.deltas.pop_front();
            }
        } else {
            m.deltas.clear();
        }
        m.state = Some(Arc::new(state));
        drop(m);
        self.inner.mixer_changed.notify_all();
    }

    /// The meters now ([`soundcraft_engine::surface::feed::meters_json`]'s `params`); only the
    /// latest is kept.
    pub fn publish_meters(&self, mut params: Value) {
        let mut m = lock(&self.inner.mixer);
        // Unchanged (stopped and quiet): nothing new to send.
        if m.last_meters.as_ref() == Some(&params) {
            return;
        }
        m.last_meters = Some(params.clone());
        m.meter_seq = m.meter_seq.wrapping_add(1);
        if let Some(o) = params.as_object_mut() {
            o.insert("seq".into(), json!(m.meter_seq));
        }
        m.meters = Some(Arc::new(json!({"event": "mixer.meters", "params": params}).to_string()));
        drop(m);
        self.inner.mixer_changed.notify_all();
    }

    /// PCs subscribed to the mixer feed now (the UI builds it only while there are some).
    pub fn mixer_subscribers(&self) -> usize {
        self.inner.mixer_subs.load(Ordering::SeqCst)
    }

    /// Mixer deltas held for subscribers catching up (at most [`MAX_MIXER_DELTAS`]).
    pub fn mixer_backlog(&self) -> usize {
        lock(&self.inner.mixer).deltas.len()
    }

    /// Paired PCs connected now (recorders, listeners and controlling PCs alike).
    pub fn connected_pcs(&self) -> usize {
        self.inner.pcs.load(Ordering::SeqCst)
    }

    /// Pushes the mixer feed to one subscriber (`mixer.subscribe`): a snapshot, then each delta in
    /// order (a fresh snapshot when it fell more than [`MAX_MIXER_DELTAS`] behind), and the latest
    /// meters at most every [`METERS_EVERY`]. A subscriber that stops reading only stalls this
    /// thread, until the link's write timeout ends it; nothing here waits on the UI.
    fn push_mixer(&self, out: &Arc<Mutex<TcpStream>>, done: &AtomicBool, stop: &AtomicBool, meters: bool) {
        self.inner.mixer_subs.fetch_add(1, Ordering::SeqCst);
        // The UI builds the feed only while someone subscribes: wake it.
        if let Some(w) = lock(&self.inner.waker).as_ref() {
            w();
        }
        let mut cursor: Option<u64> = None;
        let mut meters_seen = 0u64;
        let mut last_meters = Instant::now() - METERS_EVERY;
        let mut lines: Vec<Arc<String>> = Vec::new();
        loop {
            let quit = || done.load(Ordering::SeqCst) || stop.load(Ordering::SeqCst);
            let snapshot = {
                let g = lock(&self.inner.mixer);
                let (g, _) = self
                    .inner
                    .mixer_changed
                    .wait_timeout_while(g, Duration::from_secs(5), |m| {
                        (m.state.is_none() || Some(m.seq) == cursor) && (!meters || m.meter_seq == meters_seen) && !quit()
                    })
                    .unwrap_or_else(PoisonError::into_inner);
                if quit() {
                    break;
                }
                lines.clear();
                let mut snapshot = None;
                if let Some(state) = &g.state
                    && Some(g.seq) != cursor
                {
                    let caught_up = cursor.is_some_and(|c| g.deltas.front().is_some_and(|(first, _)| *first <= c.wrapping_add(1)));
                    if caught_up {
                        lines.extend(g.deltas.iter().filter(|(s, _)| cursor.is_some_and(|c| *s > c)).map(|(_, l)| Arc::clone(l)));
                    } else {
                        snapshot = Some((g.seq, Arc::clone(state)));
                    }
                    cursor = Some(g.seq);
                }
                if meters && g.meter_seq != meters_seen && last_meters.elapsed() >= METERS_EVERY {
                    meters_seen = g.meter_seq;
                    last_meters = Instant::now();
                    if let Some(m) = &g.meters {
                        lines.push(Arc::clone(m));
                    }
                }
                snapshot
            };
            if let Some((seq, state)) = snapshot {
                let line = json!({"event": "mixer.snapshot", "params": {"seq": seq, "state": &*state}}).to_string();
                lines.insert(0, Arc::new(line));
            }
            let mut ok = true;
            for l in &lines {
                if !write_raw(out, l) {
                    ok = false;
                    break;
                }
            }
            if !ok {
                // A stalled or gone subscriber: its link is ended, nothing waits for it.
                let _ = lock(out).shutdown(std::net::Shutdown::Both);
                break;
            }
            if lines.is_empty() && meters && meters_seen != lock(&self.inner.mixer).meter_seq {
                // Meters waiting for their turn.
                std::thread::sleep(METERS_EVERY.saturating_sub(last_meters.elapsed()).max(Duration::from_millis(1)));
            }
        }
        self.inner.mixer_subs.fetch_sub(1, Ordering::SeqCst);
    }

    fn open_link(&self, dev: &PairedDevice, report: FollowerReport, kind: LinkKind, listener: bool) -> u64 {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let ports: Vec<String> = report.ports.iter().map(|p| remote_port_name(&dev.name, p)).collect();
        let inputs: Vec<String> = report.inputs.iter().map(|p| remote_port_name(&dev.name, p)).collect();
        {
            let mut links = lock(&self.inner.links);
            links.retain(|l| l.name != dev.name || l.listener != listener);
            links.push(Link {
                id,
                name: dev.name.clone(),
                report,
                connected: true,
                last_seen: Instant::now(),
                landed: false,
                streaming: false,
                kind,
                listener,
            });
        }
        if listener {
            // Not a remote recorder: nothing to follow, no ports, nothing the booth waits for.
            return id;
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
        let mut links = lock(&self.inner.links);
        for l in links.iter_mut().filter(|l| l.id == id) {
            l.connected = false;
            log::info!("network: {} {} disconnected", if l.listener { "listening PC" } else { "remote recorder" }, l.name);
        }
        // A listener is not waited for: gone is gone.
        links.retain(|l| !(l.id == id && l.listener));
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

/// [`write_line`] for a line already made.
fn write_raw(out: &Arc<Mutex<TcpStream>>, line: &str) -> bool {
    let mut s = lock(out);
    writeln!(s, "{line}").is_ok() && s.flush().is_ok()
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
/// tap is on only while someone listens. Two encoders: one streaming while the booth is stopped
/// (`live` links), one sending only talkback then (older performers), each run only while a link
/// wants it. Ends with the hub.
fn stream_loop(weak: &std::sync::Weak<Inner>) {
    // [talkback-only, live]
    let mut encs: [Option<(u32, crate::stream::MixEncoder)>; 2] = [None, None];
    let mut blocks = Vec::new();
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let subs: Vec<StreamSub> = lock(&inner.stream_subs).clone();
        let tap = lock(&inner.tap).clone();
        match (&tap, subs.is_empty()) {
            (Some((t, rate)), false) => {
                if !t.is_on() {
                    t.clear();
                    t.set_on(true);
                }
                blocks.clear();
                t.read(&mut blocks);
                let mut dead = Vec::new();
                for (k, enc) in encs.iter_mut().enumerate() {
                    let live = k == 1;
                    if !subs.iter().any(|s| s.live == live) {
                        *enc = None;
                        continue;
                    }
                    if enc.as_ref().is_none_or(|(r, _)| r != rate) {
                        let e = if live { crate::stream::MixEncoder::live(*rate) } else { crate::stream::MixEncoder::new(*rate) };
                        *enc = Some((*rate, e));
                    }
                    let Some((_, e)) = enc.as_mut() else { continue };
                    for p in e.feed(&blocks) {
                        let line = json!({"event": "stream", "params": p});
                        for s in subs.iter().filter(|s| s.live == live) {
                            if dead.contains(&s.link) {
                                continue;
                            }
                            if !write_line(&s.out, &line) {
                                dead.push(s.link);
                            }
                        }
                    }
                }
                if !dead.is_empty() {
                    lock(&inner.stream_subs).retain(|s| !dead.contains(&s.link));
                }
            }
            _ => {
                if let Some((t, _)) = &tap {
                    t.set_on(false);
                }
                encs = [None, None];
            }
        }
        drop(inner);
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A caller's address for the booth's notices: empty when it is this PC's own (a bridge from
/// the encrypted link).
fn from_addr(addr: SocketAddr) -> String {
    if addr.ip().is_loopback() { String::new() } else { addr.ip().to_string() }
}

/// How long an invite is good for (minutes).
pub const INVITE_MINUTES: f64 = 30.0;

/// The most joins a reusable invite allows.
pub const MAX_INVITE_JOINS: u32 = 50;
/// Wrong invite secrets allowed per [`BAD_INVITE_WINDOW_S`] before every invite is refused for a
/// while (a secret is 128 random bits: this only keeps the listener from being hammered).
pub const MAX_BAD_INVITES: usize = 20;
pub const BAD_INVITE_WINDOW_S: f64 = 600.0;

/// An invite still open, as Setup › Network lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenInvite {
    pub id: String,
    /// Several devices may join with it (else one, within 30 minutes).
    pub reusable: bool,
    pub secs_left: f64,
    pub joins: u32,
    pub max_joins: u32,
}

/// An invite secret's SHA-256 (hex), as the settings keep a reusable one.
fn secret_hash(secret: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(secret.trim().as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether two secrets (or hashes) are the same, compared in full.
fn same_secret(a: &str, b: &str) -> bool {
    crate::pairing::same_pin(a, b)
}

/// Seconds since 1970.
/// "just now", "12 s ago", "3 min ago".
pub fn ago(secs: f64) -> String {
    if secs < 2.0 {
        "just now".into()
    } else if secs < 120.0 {
        format!("{secs:.0} s ago")
    } else {
        format!("{:.0} min ago", secs / 60.0)
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

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
