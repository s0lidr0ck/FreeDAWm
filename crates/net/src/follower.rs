//! A remote recorder's link to the booth it follows (Setup › Network › Follow a booth).
//!
//! One thread keeps a connection to the booth (found by name through mDNS, else at its last
//! address), retrying every couple of seconds when it is away. Over it the booth pushes its
//! transport ([`BoothTransport`]); this side sends sync pings during a take (a burst at the
//! start, then every 2 s), the live sketch (every 250 ms), a report (every second) and the
//! finished takes, one at a time, each removed from the outbox only once the booth says it
//! landed. The outbox is kept on disk, so a take survives the network, the booth and this app
//! going away. The local recording is never touched by any of this: it is the source of truth.

use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::io::{BufReader, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{Duration, Instant};

use crate::discovery::Found;
use crate::hub::{TakeClock, read_line};
use crate::proto::{BoothTransport, FollowerReport, SketchNote};
use crate::sync::{ClockMap, SyncPoint, push_point, sync_point};
use crate::take::{BoothAnchor, TakeEnvelope, TakePayload};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whom to follow, and as whom.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FollowTarget {
    pub booth_name: String,
    /// `host:port` (the last one it was found at).
    pub addr: String,
    pub token: String,
    /// This PC's name.
    pub my_name: String,
}

/// The booth's transport as it arrived.
#[derive(Debug, Clone, PartialEq)]
pub struct FollowEvent {
    pub transport: BoothTransport,
    /// When it arrived (the booth's position has moved on since, while it rolls).
    pub at: Instant,
    /// From the greeting on (re)connecting rather than a change: a take already running is
    /// joined late.
    pub joined: bool,
}

/// The take this PC is recording for the booth.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveTake {
    pub booth_take: u64,
    pub anchor: BoothAnchor,
    pub sync: Vec<SyncPoint>,
    /// What the performer heard of the booth's mix stream when: (his take clock, the booth's
    /// take clock of the sample he heard), every ~100 ms while it plays.
    pub heard: Vec<SyncPoint>,
    pub partial: bool,
    started: Instant,
    pings: u32,
}

/// An audio take's transfer file on its way (see `crate::audio`).
#[derive(Debug, Clone, Default)]
struct Upload {
    id: String,
    path: PathBuf,
    file: String,
    bytes: u64,
    hash: String,
    /// The offer went out; its answer came.
    offered: bool,
    answered: bool,
    /// What the booth has (`acked`) and what was sent so far.
    acked: u64,
    sent: u64,
}

#[derive(Debug, Clone, Default)]
struct Link {
    connected: bool,
    state: String,
    last_contact: Option<Instant>,
    booth_pc: String,
    best_rtt: Option<f64>,
}

struct Inner {
    target: Mutex<FollowTarget>,
    clock: RwLock<Option<Arc<dyn TakeClock>>>,
    events: Mutex<VecDeque<FollowEvent>>,
    link: Mutex<Link>,
    take: Mutex<Option<ActiveTake>>,
    map: Mutex<Option<(usize, ClockMap)>>,
    outbox: Mutex<Vec<TakeEnvelope>>,
    outbox_dir: Option<PathBuf>,
    in_flight: Mutex<Option<(String, Instant)>>,
    retry_after: Mutex<Option<Instant>>,
    feed: Mutex<Vec<(String, SketchNote)>>,
    /// Live audio level batches waiting to go (`feed.audio` params).
    audio_feed: Mutex<Vec<Value>>,
    upload: Mutex<Option<Upload>>,
    /// Audio takes being turned into transfer files.
    preparing: AtomicUsize,
    /// The booth's mix stream: wanted, the player's end of it (with the session rate), its
    /// decoder, and whether this connection asked for it.
    want_stream: AtomicBool,
    stream_in: Mutex<Option<(Arc<soundcraft_playback::stream::StreamIn>, u32)>>,
    decoder: Mutex<Option<crate::stream::MixDecoder>>,
    subscribed: AtomicBool,
    report: Mutex<FollowerReport>,
    pings: Mutex<HashMap<String, (f64, u64)>>,
    next_ping: AtomicU64,
    stop: AtomicBool,
    found: Mutex<Option<Arc<Mutex<Vec<Found>>>>>,
    /// How often a take pings after its first burst.
    ping_every: Mutex<Duration>,
    waker: Mutex<Option<crate::hub::Waker>>,
}

/// A running link to the booth (stopped when dropped... or with [`Follower::stop`]).
#[derive(Clone)]
pub struct Follower {
    inner: Arc<Inner>,
}

/// The pings at the start of a take, 200 ms apart, before the 2 s rhythm.
const BURST: u32 = 5;

impl Follower {
    /// Starts following `target`; `outbox_dir` keeps the takes waiting to be sent (and is read
    /// for ones left from before).
    pub fn start(target: FollowTarget, outbox_dir: Option<PathBuf>, clock: Option<Arc<dyn TakeClock>>) -> Follower {
        let outbox = outbox_dir.as_deref().map(load_outbox).unwrap_or_default();
        let f = Follower {
            inner: Arc::new(Inner {
                target: Mutex::new(target),
                clock: RwLock::new(clock),
                events: Mutex::new(VecDeque::new()),
                link: Mutex::new(Link { state: "connecting".into(), ..Link::default() }),
                take: Mutex::new(None),
                map: Mutex::new(None),
                outbox: Mutex::new(outbox),
                outbox_dir,
                in_flight: Mutex::new(None),
                retry_after: Mutex::new(None),
                feed: Mutex::new(Vec::new()),
                audio_feed: Mutex::new(Vec::new()),
                upload: Mutex::new(None),
                preparing: AtomicUsize::new(0),
                want_stream: AtomicBool::new(false),
                stream_in: Mutex::new(None),
                decoder: Mutex::new(None),
                subscribed: AtomicBool::new(false),
                report: Mutex::new(FollowerReport { state: "following".into(), ..FollowerReport::default() }),
                pings: Mutex::new(HashMap::new()),
                next_ping: AtomicU64::new(1),
                stop: AtomicBool::new(false),
                found: Mutex::new(None),
                ping_every: Mutex::new(Duration::from_secs(2)),
                waker: Mutex::new(None),
            }),
        };
        let me = f.clone();
        let _ = std::thread::Builder::new().name("freedawm-follow".into()).spawn(move || me.run());
        f.prepare_pending();
        f
    }

    /// Turns the audio takes in the outbox that have no transfer file yet into one (FLAC at the
    /// booth project's depth, see [`crate::audio::encode_transfer`]), on a thread.
    fn prepare_pending(&self) {
        let pending = lock(&self.inner.outbox).iter().any(unprepared);
        // One at a time: a running one looks for more before it ends (and the link's loop asks
        // again every second).
        if !pending || self.inner.preparing.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return;
        }
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("freedawm-take-prep".into()).spawn(move || {
            loop {
                let next = lock(&me.inner.outbox).iter().find(|e| unprepared(e)).cloned();
                let Some(env) = next else { break };
                let id = env.id.clone();
                match prepare_audio(env) {
                    Ok(env) => {
                        let project = env.audio().and_then(|a| project_of(std::path::Path::new(&a.dir)));
                        me.enqueue_in(env, project.as_deref());
                    }
                    Err(e) => {
                        log::warn!("remote take {id}: {e}");
                        // Not sendable: kept (the local take is still in its project) and reported.
                        lock(&me.inner.outbox).retain(|t| t.id != id);
                        break;
                    }
                }
            }
            me.inner.preparing.store(0, Ordering::SeqCst);
        });
        if spawned.is_err() {
            self.inner.preparing.store(0, Ordering::SeqCst);
        }
    }

    /// Hear the booth's mix (a remote performer), into the player's `stream_in` at session rate
    /// `rate` (once a frame from the UI thread; `None` without a player).
    pub fn set_stream(&self, want: bool, stream_in: Option<Arc<soundcraft_playback::stream::StreamIn>>, rate: u32) {
        self.inner.want_stream.store(want, Ordering::SeqCst);
        let mut cur = lock(&self.inner.stream_in);
        let same = match (&*cur, &stream_in) {
            (Some((a, r)), Some(b)) => Arc::ptr_eq(a, b) && *r == rate,
            (None, None) => true,
            _ => false,
        };
        if !same {
            if let Some((old, _)) = &*cur {
                old.set_on(false);
            }
            *cur = stream_in.map(|s| (s, rate));
        }
        let now = cur.clone();
        drop(cur);
        if !same {
            *lock(&self.inner.decoder) = None;
        }
        if let Some((s, _)) = &now {
            s.set_on(want && self.connected());
        }
    }

    /// The mix stream as heard now: how far behind the booth (the jitter buffer's target, ms),
    /// packets lost and late, and the booth timeline position heard (s).
    pub fn stream_health(&self) -> Option<StreamHealth> {
        // One lock at a time (the network threads take them in this order too).
        let s = lock(&self.inner.stream_in).clone();
        let rate = q_rate(&s);
        let (q, _) = s?;
        let d = lock(&self.inner.decoder);
        let d = d.as_ref()?;
        Some(StreamHealth {
            buffer_ms: d.target_ms,
            lost: d.lost,
            late: d.late,
            packets: d.packets,
            starved_ms: q.starved() as f64 * 1000.0 / f64::from(rate),
            heard_pos: d.heard_pos(q.mark()),
        })
    }

    /// Feeds the stream decoder's queue and notes what the performer heard (the network threads).
    fn pump_stream(&self) {
        let s = lock(&self.inner.stream_in).clone();
        let Some((queue, _)) = s else { return };
        let mut d = lock(&self.inner.decoder);
        let Some(dec) = d.as_mut() else { return };
        dec.pump(&queue);
        if let Some(p) = dec.heard(queue.mark())
            && let Some(t) = lock(&self.inner.take).as_mut()
            && t.heard.last().is_none_or(|l| p.local - l.local >= 0.1)
            && t.heard.len() < crate::sync::MAX_POINTS
        {
            t.heard.push(p);
        }
    }

    /// A stream packet from the booth.
    fn on_stream(&self, v: &Value) {
        let Ok(p) = serde_json::from_value::<crate::stream::StreamPacket>(v["params"].clone()) else { return };
        let s = lock(&self.inner.stream_in).clone();
        let Some((queue, rate)) = s else { return };
        {
            let mut d = lock(&self.inner.decoder);
            let dec = d.get_or_insert_with(|| crate::stream::MixDecoder::new(rate));
            dec.on_packet(p, &queue);
        }
        self.pump_stream();
    }

    /// Audio takes being turned into transfer files now.
    pub fn preparing(&self) -> bool {
        self.inner.preparing.load(Ordering::SeqCst) > 0 || lock(&self.inner.outbox).iter().any(unprepared)
    }

    /// How far the audio take on its way has gone (0..1), while one goes.
    pub fn progress(&self) -> Option<f64> {
        lock(&self.inner.upload).as_ref().filter(|u| u.bytes > 0).map(|u| u.acked as f64 / u.bytes as f64)
    }

    /// Queues a live audio level batch for the booth (`feed.audio`): booth timeline seconds.
    pub fn push_audio_feed(&self, input: &str, peaks: &[(f64, f32)], clips: &[f64], level: f32) {
        let mut f = lock(&self.inner.audio_feed);
        if f.len() < 1_000 {
            f.push(json!({"input": input, "peaks": peaks, "clips": clips, "level": level}));
        }
    }

    /// Wakes the UI thread when the booth's transport changes.
    pub fn set_waker(&self, w: Option<crate::hub::Waker>) {
        *lock(&self.inner.waker) = w;
    }

    fn push_event(&self, ev: FollowEvent) {
        lock(&self.inner.events).push_back(ev);
        if let Some(w) = lock(&self.inner.waker).as_ref() {
            w();
        }
    }

    pub fn stop(&self) {
        self.inner.stop.store(true, Ordering::SeqCst);
    }

    pub fn target(&self) -> FollowTarget {
        lock(&self.inner.target).clone()
    }

    /// Looks the booth up by name in what mDNS browsing finds.
    pub fn set_lookup(&self, found: Arc<Mutex<Vec<Found>>>) {
        *lock(&self.inner.found) = Some(found);
    }

    pub fn set_clock(&self, clock: Option<Arc<dyn TakeClock>>) {
        *self.inner.clock.write().unwrap_or_else(PoisonError::into_inner) = clock;
    }

    /// For tests: pings after the burst this often.
    pub fn set_ping_every(&self, d: Duration) {
        *lock(&self.inner.ping_every) = d;
    }

    fn now(&self) -> Option<f64> {
        self.inner.clock.read().unwrap_or_else(PoisonError::into_inner).as_ref().and_then(|c| c.now())
    }

    /// The booth's transport changes since the last call.
    pub fn drain_events(&self) -> Vec<FollowEvent> {
        lock(&self.inner.events).drain(..).collect()
    }

    pub fn connected(&self) -> bool {
        lock(&self.inner.link).connected
    }

    /// `connected`, `not reachable`, `not paired`, `looking for <booth>`…
    pub fn state(&self) -> String {
        lock(&self.inner.link).state.clone()
    }

    /// The booth's own name, once connected.
    pub fn booth_pc(&self) -> String {
        lock(&self.inner.link).booth_pc.clone()
    }

    /// Time since the booth was last heard from (`None`: never).
    pub fn since_contact(&self) -> Option<Duration> {
        lock(&self.inner.link).last_contact.map(|t| t.elapsed())
    }

    /// Half the best round trip seen (the booth's message is that old when it arrives).
    pub fn half_rtt(&self) -> f64 {
        lock(&self.inner.link).best_rtt.unwrap_or(0.0) * 0.5
    }

    /// A take starts for booth take `booth_take`.
    pub fn begin_take(&self, booth_take: u64, anchor: BoothAnchor, partial: bool) {
        *lock(&self.inner.take) =
            Some(ActiveTake { booth_take, anchor, sync: Vec::new(), heard: Vec::new(), partial, started: Instant::now(), pings: 0 });
        *lock(&self.inner.map) = None;
    }

    pub fn active_take(&self) -> Option<ActiveTake> {
        lock(&self.inner.take).clone()
    }

    /// The booth's take ended at `end` (booth seconds).
    pub fn set_end(&self, end: Option<f64>) {
        if let Some(t) = lock(&self.inner.take).as_mut() {
            t.anchor.end = end.or(t.anchor.end);
        }
    }

    /// The take ends here: its sync points and anchor, for the envelope.
    pub fn end_take(&self) -> Option<ActiveTake> {
        lock(&self.inner.take).take()
    }

    /// The booth's clock when this PC's read `local`, by the take's points so far.
    pub fn estimate_booth(&self, local: f64) -> Option<f64> {
        let take = lock(&self.inner.take);
        let pts = &take.as_ref()?.sync;
        let mut cache = lock(&self.inner.map);
        if cache.as_ref().is_none_or(|(n, _)| *n != pts.len()) {
            *cache = ClockMap::new(pts).map(|m| (pts.len(), m));
        }
        cache.as_ref().map(|(_, m)| m.map(local))
    }

    /// Queues a finished take for the booth (kept on disk until it landed), in the outbox.
    pub fn enqueue(&self, env: TakeEnvelope) {
        self.enqueue_in(env, None);
    }

    /// Queues a finished take for the booth. With the project folder of the session it was
    /// recorded in, the take itself is kept there (`<project>/Remote Takes/<id>.json`, and stays
    /// after it landed) and the outbox keeps only a pointer to it (`<id>.ptr`); without one, the
    /// outbox keeps the take. Either way it is read back if this app stops before it landed.
    pub fn enqueue_in(&self, mut env: TakeEnvelope, project: Option<&std::path::Path>) {
        let name = file_safe(&env.id);
        // An audio take's transfer file is made next to where the take is kept.
        if let TakePayload::Audio(a) = &mut env.payload
            && a.dir.is_empty()
        {
            let dir = project.map(|p| p.join(PROJECT_TAKES)).or_else(|| self.inner.outbox_dir.clone()).unwrap_or_else(std::env::temp_dir);
            a.dir = dir.to_string_lossy().into_owned();
        }
        let audio = env.audio().is_some();
        let kept = project.and_then(|dir| {
            let dir = dir.join(PROJECT_TAKES);
            let path = dir.join(format!("{name}.json"));
            let bytes = serde_json::to_vec(&env).map_err(|e| log::warn!("remote take {}: {e}", env.id)).ok()?;
            match write_atomic(&dir, &path, &bytes) {
                Ok(()) => Some(path),
                Err(e) => {
                    log::warn!("remote take {}: could not keep it in the project ({e}); the outbox keeps it", env.id);
                    None
                }
            }
        });
        if let Some(dir) = &self.inner.outbox_dir {
            let written = match &kept {
                Some(path) => serde_json::to_vec(&json!({"take": path.to_string_lossy()})).map_err(|e| e.to_string()).and_then(|b| {
                    let _ = std::fs::remove_file(dir.join(format!("{name}.json")));
                    write_atomic(dir, &dir.join(format!("{name}.{POINTER}")), &b)
                }),
                None => serde_json::to_vec(&env).map_err(|e| e.to_string()).and_then(|b| write_atomic(dir, &dir.join(format!("{name}.json")), &b)),
            };
            if let Err(e) = written {
                log::warn!("remote take {}: could not keep it on disk: {e}", env.id);
            }
        }
        {
            let mut q = lock(&self.inner.outbox);
            match q.iter_mut().find(|t| t.id == env.id) {
                Some(t) => *t = env,
                None => q.push(env),
            }
        }
        *lock(&self.inner.retry_after) = None;
        if audio {
            self.prepare_pending();
        }
    }

    /// Takes waiting to be sent.
    pub fn queued(&self) -> usize {
        lock(&self.inner.outbox).len()
    }

    /// A take on its way now.
    pub fn sending(&self) -> bool {
        lock(&self.inner.in_flight).is_some()
    }

    pub fn push_feed(&self, port: &str, notes: &[SketchNote]) {
        let mut f = lock(&self.inner.feed);
        if f.len() < 100_000 {
            f.extend(notes.iter().map(|n| (port.to_string(), *n)));
        }
    }

    pub fn set_report(&self, r: FollowerReport) {
        *lock(&self.inner.report) = r;
    }

    fn stopped(&self) -> bool {
        self.inner.stop.load(Ordering::SeqCst)
    }

    fn set_link(&self, f: impl FnOnce(&mut Link)) {
        f(&mut lock(&self.inner.link));
    }

    fn sleep(&self, d: Duration) {
        let end = Instant::now() + d;
        while Instant::now() < end && !self.stopped() {
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn resolve(&self) -> Option<SocketAddr> {
        let mut t = lock(&self.inner.target);
        if let Some(found) = lock(&self.inner.found).as_ref() {
            let f = found.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(hit) = f.iter().find(|x| x.name.eq_ignore_ascii_case(&t.booth_name) && !t.booth_name.is_empty()) {
                t.addr = hit.addr.clone();
            }
        }
        t.addr.to_socket_addrs().ok()?.find(SocketAddr::is_ipv4)
    }

    fn run(&self) {
        while !self.stopped() {
            let Some(addr) = self.resolve() else {
                let name = self.target().booth_name;
                self.set_link(|l| {
                    l.connected = false;
                    l.state = format!("looking for {name}");
                });
                self.sleep(Duration::from_secs(1));
                continue;
            };
            match self.session(addr) {
                Ok(()) => {}
                Err(e) => {
                    self.set_link(|l| {
                        l.connected = false;
                        if l.state != "not paired" {
                            l.state = "not reachable".into();
                        }
                    });
                    log::debug!("follow: {e}");
                }
            }
            self.set_link(|l| l.connected = false);
            *lock(&self.inner.in_flight) = None;
            // No stream while away: silence, not the last of it on a loop.
            if let Some((q, _)) = &*lock(&self.inner.stream_in) {
                q.set_on(false);
            }
            // Offered again on the next connection: the booth says how much it has.
            if let Some(u) = lock(&self.inner.upload).as_mut() {
                u.offered = false;
                u.answered = false;
            }
            self.sleep(Duration::from_secs(2));
        }
    }

    /// One connection, until it drops.
    fn session(&self, addr: SocketAddr) -> Result<(), String> {
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| e.to_string())?;
        let _ = stream.set_nodelay(true);
        stream.set_read_timeout(Some(Duration::from_secs(15))).map_err(|e| e.to_string())?;
        stream.set_write_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
        let out = Arc::new(Mutex::new(stream.try_clone().map_err(|e| e.to_string())?));
        let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        let t = self.target();
        let report = lock(&self.inner.report).clone();
        send(
            &out,
            &json!({"id": "hello", "method": "hello", "params": {"token": t.token, "name": t.my_name, "role": "follower", "report": report}}),
        )?;
        let mut buf = Vec::new();
        if !read_line(&mut reader, &mut buf, 1 << 20)? {
            return Err("closed".into());
        }
        let hello: Value = serde_json::from_slice(&buf).map_err(|e| e.to_string())?;
        if hello["ok"] != true {
            let unpaired = hello["pair"] == true;
            self.set_link(|l| l.state = if unpaired { "not paired".into() } else { "refused".into() });
            return Err(hello["error"].as_str().unwrap_or("refused").to_string());
        }
        let booth_pc = hello["result"]["pc"].as_str().unwrap_or("").to_string();
        self.set_link(|l| {
            l.connected = true;
            l.state = "connected".into();
            l.last_contact = Some(Instant::now());
            l.booth_pc = booth_pc;
        });
        if let Ok(tr) = serde_json::from_value::<BoothTransport>(hello["result"]["transport"].clone()) {
            self.push_event(FollowEvent { transport: tr, at: Instant::now(), joined: true });
        }
        let done = Arc::new(AtomicBool::new(false));
        let reader_thread = {
            let me = self.clone();
            let done = Arc::clone(&done);
            std::thread::Builder::new()
                .name("freedawm-follow-read".into())
                .spawn(move || {
                    me.read_loop(reader);
                    done.store(true, Ordering::SeqCst);
                })
                .map_err(|e| e.to_string())?
        };
        let result = self.write_loop(&out, &done);
        let _ = stream.shutdown(std::net::Shutdown::Both);
        let _ = reader_thread.join();
        result
    }

    fn read_loop(&self, mut reader: BufReader<TcpStream>) {
        let mut buf = Vec::new();
        while !self.stopped() {
            match read_line(&mut reader, &mut buf, 1 << 20) {
                Ok(true) => {}
                _ => return,
            }
            // The answer's arrival on this PC's clock, before parsing (a sync pong).
            let t3 = self.now();
            let Ok(v) = serde_json::from_slice::<Value>(&buf) else { continue };
            self.set_link(|l| l.last_contact = Some(Instant::now()));
            if v["event"] == "stream" {
                self.on_stream(&v);
                continue;
            }
            if v["event"] == "transport" {
                if let Ok(tr) = serde_json::from_value::<BoothTransport>(v["params"].clone()) {
                    self.push_event(FollowEvent { transport: tr, at: Instant::now(), joined: false });
                }
                continue;
            }
            let Some(id) = v["id"].as_str() else { continue };
            if id.starts_with("p:") {
                let sent = lock(&self.inner.pings).remove(id);
                if let (Some((t0, take)), Some(t3), true) = (sent, t3, v["ok"] == true) {
                    let (b1, b2) = (v["result"]["b1"].as_f64(), v["result"]["b2"].as_f64());
                    if let (Some(b1), Some(b2)) = (b1, b2)
                        && let Some(p) = sync_point(t0, b1, b2, t3)
                    {
                        self.set_link(|l| l.best_rtt = Some(l.best_rtt.map_or(p.rtt, |b| b.min(p.rtt))));
                        if let Some(t) = lock(&self.inner.take).as_mut().filter(|t| t.booth_take == take) {
                            push_point(&mut t.sync, p);
                        }
                    }
                }
            } else if let Some(rest) = id.strip_prefix("u:") {
                self.upload_answer(rest, &v);
            } else if let Some(take) = id.strip_prefix("t:") {
                let mut flight = lock(&self.inner.in_flight);
                if flight.as_ref().is_some_and(|(f, _)| f == take) {
                    *flight = None;
                }
                drop(flight);
                if v["ok"] == true {
                    let gone: Vec<TakeEnvelope> = {
                        let mut q = lock(&self.inner.outbox);
                        let gone = q.iter().filter(|t| t.id == take).cloned().collect();
                        q.retain(|t| t.id != take);
                        gone
                    };
                    // The transfer file has done its job (the local take stays in its project).
                    for a in gone.iter().filter_map(TakeEnvelope::audio) {
                        let _ = std::fs::remove_file(std::path::Path::new(&a.dir).join(&a.file));
                    }
                    if lock(&self.inner.upload).as_ref().is_some_and(|u| u.id == take) {
                        *lock(&self.inner.upload) = None;
                    }
                    if let Some(dir) = &self.inner.outbox_dir {
                        // The pointer or the take the outbox kept; a take kept in its project stays there.
                        let _ = std::fs::remove_file(dir.join(format!("{}.json", file_safe(take))));
                        let _ = std::fs::remove_file(dir.join(format!("{}.{POINTER}", file_safe(take))));
                    }
                    log::info!("remote take {take} landed on the host");
                } else {
                    log::warn!("remote take {take}: {}", v["error"]);
                    *lock(&self.inner.retry_after) = Some(Instant::now() + Duration::from_secs(10));
                }
            }
        }
    }

    fn write_loop(&self, out: &Arc<Mutex<TcpStream>>, done: &AtomicBool) -> Result<(), String> {
        let mut last_ping = Instant::now() - Duration::from_secs(60);
        let mut last_feed = Instant::now();
        let mut last_report = Instant::now() - Duration::from_secs(60);
        self.inner.subscribed.store(false, Ordering::SeqCst);
        while !self.stopped() && !done.load(Ordering::SeqCst) {
            // The booth's mix: asked for (or not) on this connection; fed to the audio thread.
            let want = self.inner.want_stream.load(Ordering::SeqCst) && lock(&self.inner.stream_in).is_some();
            if want != self.inner.subscribed.load(Ordering::SeqCst) {
                self.inner.subscribed.store(want, Ordering::SeqCst);
                if let Some((q, _)) = &*lock(&self.inner.stream_in) {
                    q.set_on(want);
                }
                send(out, &json!({"id": "s", "method": "stream.subscribe", "params": {"on": want}}))?;
            }
            if want {
                self.pump_stream();
            }
            // Sync pings while a take records and this PC's transport rolls.
            let ping_take = lock(&self.inner.take).as_ref().map(|t| (t.booth_take, t.pings));
            if let Some((take, n)) = ping_take {
                let every = if n < BURST { Duration::from_millis(200) } else { *lock(&self.inner.ping_every) };
                if last_ping.elapsed() >= every
                    && let Some(t0) = self.now()
                {
                    let id = format!("p:{}", self.inner.next_ping.fetch_add(1, Ordering::Relaxed));
                    {
                        let mut pings = lock(&self.inner.pings);
                        // Unanswered ones are forgotten.
                        if pings.len() > 64 {
                            pings.clear();
                        }
                        pings.insert(id.clone(), (t0, take));
                    }
                    send(out, &json!({"id": id, "method": "sync.ping", "params": {"t0": t0}}))?;
                    if let Some(t) = lock(&self.inner.take).as_mut() {
                        t.pings = t.pings.saturating_add(1);
                    }
                    last_ping = Instant::now();
                }
            }
            if last_feed.elapsed() >= Duration::from_millis(250) {
                last_feed = Instant::now();
                let batch: Vec<(String, SketchNote)> = std::mem::take(&mut *lock(&self.inner.feed));
                let mut ports: Vec<&str> = batch.iter().map(|(p, _)| p.as_str()).collect();
                ports.dedup();
                ports.sort_unstable();
                ports.dedup();
                for port in ports {
                    let notes: Vec<SketchNote> = batch.iter().filter(|(p, _)| p == port).map(|(_, n)| *n).collect();
                    send(out, &json!({"method": "feed", "params": {"port": port, "notes": notes}}))?;
                }
                let levels: Vec<Value> = std::mem::take(&mut *lock(&self.inner.audio_feed));
                for b in levels {
                    send(out, &json!({"method": "feed.audio", "params": b}))?;
                }
            }
            if last_report.elapsed() >= Duration::from_secs(1) {
                last_report = Instant::now();
                self.prepare_pending();
                let mut r = lock(&self.inner.report).clone();
                r.sending = self.progress();
                send(out, &json!({"method": "follower.report", "params": r}))?;
            }
            let can_send = lock(&self.inner.in_flight).as_ref().is_none_or(|(_, at)| at.elapsed() > Duration::from_secs(90))
                && lock(&self.inner.retry_after).is_none_or(|t| Instant::now() >= t);
            if can_send {
                let next = lock(&self.inner.outbox).iter().find(|e| !unprepared(e)).cloned();
                // An audio take's file goes first, in chunks; its envelope once the booth has it all.
                let next = match next {
                    Some(env) if env.audio().is_some() => {
                        if self.upload_step(out, &env)? {
                            Some(env)
                        } else {
                            None
                        }
                    }
                    other => other,
                };
                if let Some(env) = next {
                    *lock(&self.inner.in_flight) = Some((env.id.clone(), Instant::now()));
                    let id = format!("t:{}", env.id);
                    send(out, &json!({"id": id, "method": "take.transfer", "params": env}))?;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
}

impl Follower {
    /// Moves an audio take's transfer file along: offers it, then sends chunks a few ahead of the
    /// booth's answers. `true` once the booth has it all (its envelope can follow).
    fn upload_step(&self, out: &Arc<Mutex<TcpStream>>, env: &TakeEnvelope) -> Result<bool, String> {
        let Some(a) = env.audio() else { return Ok(true) };
        let mut guard = lock(&self.inner.upload);
        if guard.as_ref().is_none_or(|u| u.id != env.id) {
            *guard = Some(Upload {
                id: env.id.clone(),
                path: std::path::Path::new(&a.dir).join(&a.file),
                file: a.file.clone(),
                bytes: a.bytes,
                hash: a.hash.clone(),
                ..Upload::default()
            });
        }
        let Some(u) = guard.as_mut() else { return Ok(false) };
        if !u.offered {
            u.offered = true;
            let msg = json!({"id": format!("u:o:{}", u.id), "method": "take.offer", "params": {"id": u.id, "file": u.file, "bytes": u.bytes, "hash": u.hash}});
            drop(guard);
            send(out, &msg)?;
            return Ok(false);
        }
        if !u.answered {
            return Ok(false);
        }
        if u.acked >= u.bytes {
            return Ok(true);
        }
        let window = crate::audio::WINDOW * crate::audio::CHUNK as u64;
        let mut msgs = Vec::new();
        while u.sent < u.bytes && u.sent.saturating_sub(u.acked) < window {
            let n = (u.bytes - u.sent).min(crate::audio::CHUNK as u64);
            let data = read_at(&u.path, u.sent, n)?;
            let end = u.sent + data.len() as u64;
            if data.is_empty() {
                return Err(format!("{}: shorter than it should be", u.path.display()));
            }
            msgs.push(json!({"id": format!("u:c:{end}:{}", u.id), "method": "take.chunk", "params": {"id": u.id, "offset": u.sent, "data": crate::audio::b64_encode(&data)}}));
            u.sent = end;
        }
        drop(guard);
        for m in msgs {
            send(out, &m)?;
        }
        Ok(false)
    }

    /// The booth's answer to an offer (`o:<id>`) or a chunk (`c:<end>:<id>`): how much it has.
    fn upload_answer(&self, rest: &str, v: &Value) {
        let mut guard = lock(&self.inner.upload);
        let (kind, rest) = rest.split_once(':').unwrap_or((rest, ""));
        let (end, id) = if kind == "c" {
            let (e, id) = rest.split_once(':').unwrap_or(("0", rest));
            (e.parse::<u64>().unwrap_or(0), id)
        } else {
            (0, rest)
        };
        let Some(u) = guard.as_mut().filter(|u| u.id == id) else { return };
        if v["ok"] != true {
            log::warn!("remote take {id}: {}", v["error"]);
            // Offered again after a pause: the booth says where to go on from.
            u.offered = false;
            u.answered = false;
            u.sent = u.acked;
            drop(guard);
            *lock(&self.inner.retry_after) = Some(Instant::now() + Duration::from_secs(2));
            return;
        }
        let have = v["result"]["have"].as_u64().unwrap_or(0).min(u.bytes);
        u.acked = have;
        if kind == "o" {
            u.answered = true;
            u.sent = have;
        } else if have < end {
            // That chunk didn't land where it should: go back to what the booth has.
            u.sent = u.sent.min(have);
        }
    }
}

/// `n` bytes of `path` from `at`.
fn read_at(path: &std::path::Path, at: u64, n: u64) -> Result<Vec<u8>, String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
    let mut buf = Vec::with_capacity(usize::try_from(n).unwrap_or(0));
    f.take(n).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    Ok(buf)
}

/// An audio take without its transfer file yet.
fn unprepared(e: &TakeEnvelope) -> bool {
    e.audio().is_some_and(|a| a.bytes == 0)
}

/// The project folder of a transfer file's folder (`<project>/Remote Takes`).
fn project_of(dir: &std::path::Path) -> Option<PathBuf> {
    (dir.file_name().and_then(|n| n.to_str()) == Some(PROJECT_TAKES)).then(|| dir.parent().map(std::path::Path::to_path_buf)).flatten()
}

/// Makes an audio take's transfer file from its local take file (see [`crate::audio`]).
fn prepare_audio(mut env: TakeEnvelope) -> Result<TakeEnvelope, String> {
    let TakePayload::Audio(a) = &mut env.payload else { return Ok(env) };
    let bytes = std::fs::read(&a.source).map_err(|e| format!("{}: {e}", a.source))?;
    let (_, buf) = soundcraft_audio_io::decode(&bytes, Some("wav")).map_err(|e| format!("{}: {e}", a.source))?;
    drop(bytes);
    let bits = if a.bits == 0 { 24 } else { a.bits };
    let data = crate::audio::encode_transfer(&buf, bits)?;
    let rate = f64::from(buf.sample_rate.max(1));
    a.clips = crate::audio::clip_frames(&buf).into_iter().map(|f| a.t0 + f as f64 / rate).collect();
    a.frames = buf.channels.iter().map(Vec::len).max().unwrap_or(0) as u64;
    a.rate = buf.sample_rate;
    a.channels = u16::try_from(buf.channels.len()).unwrap_or(u16::MAX);
    a.bits = bits;
    a.hash = crate::audio::hash(&data);
    a.bytes = data.len() as u64;
    let dir = PathBuf::from(&a.dir);
    write_atomic(&dir, &dir.join(&a.file), &data)?;
    Ok(env)
}

/// The mix stream as the performer hears it (see [`Follower::stream_health`]).
#[derive(Debug, Clone, PartialEq)]
pub struct StreamHealth {
    pub buffer_ms: f64,
    pub lost: u64,
    pub late: u64,
    pub packets: u64,
    pub starved_ms: f64,
    pub heard_pos: Option<f64>,
}

fn q_rate(s: &Option<(Arc<soundcraft_playback::stream::StreamIn>, u32)>) -> u32 {
    s.as_ref().map_or(48_000, |(_, r)| (*r).max(1))
}

/// One request to a booth on a fresh connection (pairing), its `result`.
fn one_call(addr: &str, msg: &Value) -> Result<Value, String> {
    let a = addr.to_socket_addrs().map_err(|e| e.to_string())?.find(SocketAddr::is_ipv4).ok_or("no address")?;
    let mut s = TcpStream::connect_timeout(&a, Duration::from_secs(3)).map_err(|e| format!("cannot reach the host: {e}"))?;
    // Over the internet the first call also makes the connection (hole punching): give it time.
    s.set_read_timeout(Some(Duration::from_secs(20))).map_err(|e| e.to_string())?;
    writeln!(s, "{msg}").map_err(|e| e.to_string())?;
    let mut r = BufReader::new(s);
    let mut buf = Vec::new();
    read_line(&mut r, &mut buf, 1 << 20)?;
    let v: Value = serde_json::from_slice(&buf).map_err(|e| e.to_string())?;
    if v["ok"] == true { Ok(v["result"].clone()) } else { Err(v["error"].as_str().unwrap_or("refused").to_string()) }
}

/// Asks the booth at `addr` to pair: it shows a code on its screen. Returns the request's id.
pub fn pair_request(addr: &str, my_name: &str) -> Result<String, String> {
    let r = one_call(addr, &json!({"id": 1, "method": "pair.request", "params": {"name": my_name}}))?;
    r["request"].as_str().map(str::to_string).ok_or_else(|| "no request id".to_string())
}

/// Pairs with an invite's one-time secret (over the internet bridge at `addr`): the token and the
/// booth's own name.
pub fn pair_invite(addr: &str, secret: &str, my_name: &str) -> Result<(String, String), String> {
    let r = one_call(addr, &json!({"id": 1, "method": "pair.invite", "params": {"secret": secret, "name": my_name}}))?;
    let token = r["token"].as_str().ok_or("no token")?.to_string();
    Ok((token, r["pc"].as_str().unwrap_or("").to_string()))
}

/// The code from the booth's screen: the token (and the booth's own name).
pub fn pair_confirm(addr: &str, request: &str, code: &str) -> Result<(String, String), String> {
    let r = one_call(addr, &json!({"id": 1, "method": "pair.confirm", "params": {"request": request, "code": code}}))?;
    let token = r["token"].as_str().ok_or("no token")?.to_string();
    Ok((token, r["pc"].as_str().unwrap_or("").to_string()))
}

fn send(out: &Arc<Mutex<TcpStream>>, v: &Value) -> Result<(), String> {
    let mut s = lock(out);
    writeln!(s, "{v}").and_then(|_| s.flush()).map_err(|e| e.to_string())
}

pub(crate) fn file_safe(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// The folder in a project that keeps the remote takes recorded in it.
pub const PROJECT_TAKES: &str = "Remote Takes";
/// The extension of an outbox entry that points at a take kept in its project.
const POINTER: &str = "ptr";

/// Writes `bytes` to `path` (in `dir`, made first) through a temporary file.
fn write_atomic(dir: &std::path::Path, path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("{}: {e}", path.display())
    })
}

/// The takes left in the outbox: kept there (`<id>.json`), or pointed at in their project
/// (`<id>.ptr`). A pointer whose take is gone (the project moved or deleted) is reported and kept.
fn load_outbox(dir: &std::path::Path) -> Vec<TakeEnvelope> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<TakeEnvelope> = Vec::new();
    for e in rd.flatten() {
        let path = e.path();
        let take = match path.extension().and_then(|x| x.to_str()) {
            Some("json") => Some(path.clone()),
            Some(POINTER) => {
                std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|p| p["take"].as_str().map(PathBuf::from))
            }
            _ => None,
        };
        let Some(take) = take else { continue };
        match std::fs::read(&take).ok().and_then(|b| serde_json::from_slice::<TakeEnvelope>(&b).ok()) {
            Some(env) if !v.iter().any(|t| t.id == env.id) => v.push(env),
            Some(_) => {}
            None => log::warn!("remote take {}: cannot read it", take.display()),
        }
    }
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}
