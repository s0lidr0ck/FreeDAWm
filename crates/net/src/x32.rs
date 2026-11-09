//! A Behringer X32 (or Midas M32) as a control surface, over its OSC protocol on UDP port 10023
//! (spec `docs/superpowers/specs/2026-11-09-x32-bus-control-design.md`).
//!
//! The console's faders control this session through one bus's sends: on the console, *Sends on
//! Fader* with that bus selected puts each channel's send level to the bus on its fader. Pick a
//! bus patched to no output and the console's own mix (front of house, monitors) never changes:
//! the bus is only a controller. Channel `N`'s send level moves the session's `N`th track; the
//! bus's own master fader moves the session's master.
//!
//! This PC asks the console for changes with `/xremote` (renewed every 8 s; the console forgets a
//! client after 10) and checks it is there with `/info`. One way for now: console to session.
//!
//! The pure parts (OSC messages, the fader law, the addresses) build everywhere; the link is
//! native only.

use serde::{Deserialize, Serialize};

/// The console's OSC port.
pub const PORT: u16 = 10023;

/// The X32 settings (Setup › X32 Control), kept with the network settings (`network.json`, `x32`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct X32Config {
    /// Follow the console.
    #[serde(default)]
    pub enabled: bool,
    /// The console's address (`192.168.1.50`, or with a port).
    #[serde(default)]
    pub address: String,
    /// The bus whose sends are the controller (1–16); for a stereo pair, either of its two.
    #[serde(default = "default_bus")]
    pub bus: u8,
    /// The controller is a stereo pair of buses (linked on the desk: 1–2, 3–4 … 15–16): each
    /// channel's send then has a pan too, which moves the track's pan.
    #[serde(default)]
    pub stereo: bool,
    /// That bus's master fader moves the session's master.
    #[serde(default = "yes")]
    pub master: bool,
}

fn default_bus() -> u8 {
    16
}

fn yes() -> bool {
    true
}

impl Default for X32Config {
    fn default() -> Self {
        X32Config { enabled: false, address: String::new(), bus: 16, master: true, stereo: false }
    }
}

impl X32Config {
    /// The bus the console speaks for the controller: the bus, or a stereo pair's odd bus (a
    /// linked pair's sends and master are the odd bus's).
    pub fn osc_bus(&self) -> u8 {
        let b = self.bus.clamp(1, 16);
        if self.stereo && b.is_multiple_of(2) { b - 1 } else { b }
    }

    /// `Bus 16`, or `Buses 15–16`.
    pub fn label(&self) -> String {
        let b = self.osc_bus();
        if self.stereo { format!("Buses {b}–{}", b + 1) } else { format!("Bus {b}") }
    }
}

/// An OSC argument.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Float(f32),
    Int(i32),
    Str(String),
    /// A blob (the meters): skipped here.
    Blob(Vec<u8>),
}

/// An OSC message.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub addr: String,
    pub args: Vec<Arg>,
}

fn pad4(v: &mut Vec<u8>) {
    while !v.len().is_multiple_of(4) {
        v.push(0);
    }
}

fn put_str(v: &mut Vec<u8>, s: &str) {
    v.extend_from_slice(s.as_bytes());
    v.push(0);
    pad4(v);
}

/// The message as an OSC packet.
pub fn encode(m: &Message) -> Vec<u8> {
    let mut v = Vec::with_capacity(32);
    put_str(&mut v, &m.addr);
    let mut tags = String::from(",");
    for a in &m.args {
        tags.push(match a {
            Arg::Float(_) => 'f',
            Arg::Int(_) => 'i',
            Arg::Str(_) => 's',
            Arg::Blob(_) => 'b',
        });
    }
    put_str(&mut v, &tags);
    for a in &m.args {
        match a {
            Arg::Float(f) => v.extend_from_slice(&f.to_be_bytes()),
            Arg::Int(i) => v.extend_from_slice(&i.to_be_bytes()),
            Arg::Str(s) => put_str(&mut v, s),
            Arg::Blob(b) => {
                v.extend_from_slice(&u32::try_from(b.len()).unwrap_or(0).to_be_bytes());
                v.extend_from_slice(b);
                pad4(&mut v);
            }
        }
    }
    v
}

/// A string at `at` (null-terminated, padded to 4): the string and where the next item starts.
fn get_str(b: &[u8], at: usize) -> Option<(String, usize)> {
    let rest = b.get(at..)?;
    let end = rest.iter().position(|&c| c == 0)?;
    let s = std::str::from_utf8(rest.get(..end)?).ok()?.to_string();
    let next = at.checked_add(end)?.checked_add(1)?;
    let next = next.checked_add((4 - next % 4) % 4)?;
    Some((s, next))
}

fn get4(b: &[u8], at: usize) -> Option<[u8; 4]> {
    b.get(at..at.checked_add(4)?)?.try_into().ok()
}

/// The messages in an OSC packet (a bundle's, in order; at most 64, nested at most 4 deep).
/// Anything malformed is dropped.
pub fn decode(b: &[u8]) -> Vec<Message> {
    let mut out = Vec::new();
    decode_into(b, 0, &mut out);
    out
}

fn decode_into(b: &[u8], depth: u8, out: &mut Vec<Message>) {
    if depth > 4 || out.len() >= 64 {
        return;
    }
    if b.starts_with(b"#bundle\0") {
        // `#bundle`, an 8-byte time tag, then (size, element)…
        let mut at = 16;
        while let Some(size) = get4(b, at) {
            let n = usize::try_from(u32::from_be_bytes(size)).unwrap_or(usize::MAX);
            let Some(el) = at.checked_add(4).and_then(|s| Some((s, s.checked_add(n)?))).and_then(|(s, e)| b.get(s..e)) else { return };
            decode_into(el, depth + 1, out);
            at = at.saturating_add(4).saturating_add(n);
        }
        return;
    }
    if let Some(m) = decode_message(b) {
        out.push(m);
    }
}

fn decode_message(b: &[u8]) -> Option<Message> {
    let (addr, at) = get_str(b, 0)?;
    if !addr.starts_with('/') {
        return None;
    }
    let Some((tags, mut at)) = get_str(b, at) else { return Some(Message { addr, args: Vec::new() }) };
    let mut args = Vec::new();
    for t in tags.strip_prefix(',')?.chars().take(32) {
        match t {
            'f' => {
                args.push(Arg::Float(f32::from_be_bytes(get4(b, at)?)));
                at += 4;
            }
            'i' => {
                args.push(Arg::Int(i32::from_be_bytes(get4(b, at)?)));
                at += 4;
            }
            's' => {
                let (s, next) = get_str(b, at)?;
                args.push(Arg::Str(s));
                at = next;
            }
            'b' => {
                let n = usize::try_from(u32::from_be_bytes(get4(b, at)?)).ok()?;
                let start = at.checked_add(4)?;
                let end = start.checked_add(n)?;
                args.push(Arg::Blob(b.get(start..end)?.to_vec()));
                at = end.checked_add((4 - end % 4) % 4)?;
            }
            _ => return None,
        }
    }
    Some(Message { addr, args })
}

/// A message with no arguments (a query, `/xremote`, `/info`).
pub fn msg(addr: &str) -> Message {
    Message { addr: addr.to_string(), args: Vec::new() }
}

/// The X32's fader law: a fader or send level 0..1 in dB (four straight pieces; 0 is -∞,
/// 0.75 is 0 dB, 1 is +10 dB).
pub fn fader_to_db(f: f32) -> f32 {
    let f = if f.is_finite() { f.clamp(0.0, 1.0) } else { 0.0 };
    if f <= 0.0 {
        f32::NEG_INFINITY
    } else if f >= 0.5 {
        f * 40.0 - 30.0
    } else if f >= 0.25 {
        f * 80.0 - 50.0
    } else if f >= 0.0625 {
        f * 160.0 - 70.0
    } else {
        f * 480.0 - 90.0
    }
}

/// [`fader_to_db`] the other way (below -90 dB: 0).
pub fn db_to_fader(db: f32) -> f32 {
    if !db.is_finite() || db <= -90.0 {
        return 0.0;
    }
    let f = if db < -60.0 {
        (db + 90.0) / 480.0
    } else if db < -30.0 {
        (db + 70.0) / 160.0
    } else if db < -10.0 {
        (db + 50.0) / 80.0
    } else {
        (db + 30.0) / 40.0
    };
    f.clamp(0.0, 1.0)
}

/// What a console change means here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Move {
    /// Channel `channel` (1–32)'s send level to the controller bus, in dB (-∞ as -144).
    Strip { channel: u8, db: f32 },
    /// The controller bus's master fader, in dB.
    Master { db: f32 },
    /// Channel `channel`'s send pan to a stereo controller pair: -1 (left) to 1 (right).
    Pan { channel: u8, pan: f32 },
}

/// Reads a console message for the controller `bus` (1–16, the odd bus of a stereo pair): a
/// channel's send level to it (and, for a `stereo` pair, its send pan), or its master fader.
/// Anything else is `None`.
pub fn read_move(m: &Message, bus: u8, stereo: bool) -> Option<Move> {
    let value = match m.args.first()? {
        Arg::Float(f) if f.is_finite() => *f,
        _ => return None,
    };
    let db = fader_to_db(value);
    let db = if db.is_finite() { db } else { -144.0 };
    let parts: Vec<&str> = m.addr.trim_start_matches('/').split('/').collect();
    match parts.as_slice() {
        ["ch", ch, "mix", b, "level"] => {
            let ch: u8 = ch.parse().ok()?;
            let b: u8 = b.parse().ok()?;
            ((1..=32).contains(&ch) && b == bus).then_some(Move::Strip { channel: ch, db })
        }
        ["ch", ch, "mix", b, "pan"] if stereo => {
            let ch: u8 = ch.parse().ok()?;
            let b: u8 = b.parse().ok()?;
            ((1..=32).contains(&ch) && b == bus).then_some(Move::Pan { channel: ch, pan: (value.clamp(0.0, 1.0) * 2.0 - 1.0) })
        }
        ["bus", b, "mix", "fader"] => {
            let b: u8 = b.parse().ok()?;
            (b == bus).then_some(Move::Master { db })
        }
        _ => None,
    }
}

/// The console's answer to `/info`: `X32 4.06` (its model and firmware).
pub fn read_info(m: &Message) -> Option<String> {
    if m.addr != "/info" {
        return None;
    }
    let s: Vec<&str> = m.args.iter().filter_map(|a| if let Arg::Str(s) = a { Some(s.as_str()) } else { None }).collect();
    match s.as_slice() {
        [_, _, model, version, ..] => Some(format!("{model} {version}")),
        [.., last] => Some((*last).to_string()),
        [] => Some("console".into()),
    }
}

/// The session's tracks the console's channels move, in channel order: every track from the top
/// but the master, folders and video tracks.
pub fn strip_tracks(s: &soundcraft_model::Session) -> Vec<soundcraft_model::TrackId> {
    use soundcraft_model::TrackKind;
    s.tracks.iter().filter(|t| !matches!(t.kind, TrackKind::Master | TrackKind::Folder | TrackKind::Video)).map(|t| t.id).collect()
}

/// Moves made within this long of each other are one gesture: one undo step.
pub const GESTURE_GAP: std::time::Duration = std::time::Duration::from_secs(1);

/// Puts console moves into the session, as fader drags here are (`mix.volume`, merged into one
/// undo step per gesture, so automation writing takes them as touches). Only each control's last
/// value counts. A channel past the session's tracks moves nothing. Returns the moves applied.
#[derive(Debug, Default)]
pub struct Apply {
    gesture: u64,
    last: Option<std::time::Instant>,
}

impl Apply {
    pub fn apply(&mut self, e: &mut soundcraft_engine::Engine, moves: &[Move], master: bool, now: std::time::Instant) -> usize {
        if moves.is_empty() {
            return 0;
        }
        if self.last.is_none_or(|t| now.duration_since(t) >= GESTURE_GAP) {
            self.gesture += 1;
        }
        self.last = Some(now);
        let strips = strip_tracks(e.session());
        let master_id = e.session().master().map(|t| t.id);
        // The last value of each control, in the order they first moved.
        let mut targets: Vec<(soundcraft_model::TrackId, f32)> = Vec::new();
        let mut pans: Vec<(soundcraft_model::TrackId, f32)> = Vec::new();
        let strip = |ch: u8| strips.get(usize::from(ch).saturating_sub(1)).copied();
        for m in moves {
            let (list, target) = match *m {
                Move::Strip { channel, db } => (&mut targets, strip(channel).map(|t| (t, db))),
                Move::Master { db } if master => (&mut targets, master_id.map(|t| (t, db))),
                Move::Master { .. } => continue,
                Move::Pan { channel, pan } => (&mut pans, strip(channel).map(|t| (t, pan))),
            };
            if let Some((t, v)) = target {
                match list.iter_mut().find(|x| x.0 == t) {
                    Some(x) => x.1 = v,
                    None => list.push((t, v)),
                }
            }
        }
        let key = format!("x32:{}", self.gesture);
        let mut n = 0;
        for (t, pan) in pans {
            n += usize::from(set_pan(e, t, pan, &key));
        }
        for (t, db) in targets {
            let db = db.clamp(-144.0, 12.0);
            if e.session().track(t).is_some_and(|tr| (tr.mixer.volume_db - db).abs() < 0.001) {
                continue;
            }
            match e.execute_merged("mix.volume", &serde_json::json!({"track": t.0, "db": db}), &key) {
                Ok(_) => n += 1,
                Err(err) => log::debug!("x32: {err}"),
            }
        }
        n
    }
}

/// Pans a track to `pan` (-1..1): a mono track's pan; a stereo track's two pans moved together,
/// keeping their width where it fits. True when something changed.
fn set_pan(e: &mut soundcraft_engine::Engine, t: soundcraft_model::TrackId, pan: f32, key: &str) -> bool {
    let Some(cur) = e.session().track(t).map(|tr| tr.mixer.pan.clone()) else { return false };
    let wanted: Vec<f32> = match cur.as_slice() {
        [_] => vec![pan],
        [l, r] => {
            // Its width kept, narrowed where it would go past a side (a full-width track panned
            // right narrows as a balance does).
            let half = ((r - l) * 0.5).clamp(-1.0, 1.0);
            let room = 1.0 - pan.abs();
            let half = half.clamp(-room, room);
            vec![(pan - half).clamp(-1.0, 1.0), (pan + half).clamp(-1.0, 1.0)]
        }
        _ => return false,
    };
    let mut changed = false;
    for (i, (now, to)) in cur.iter().zip(&wanted).enumerate() {
        if (now - to).abs() < 0.001 {
            continue;
        }
        match e.execute_merged("mix.pan", &serde_json::json!({"track": t.0, "pan": to, "index": i}), key) {
            Ok(_) => changed = true,
            Err(err) => log::debug!("x32: {err}"),
        }
    }
    changed
}

#[cfg(not(target_arch = "wasm32"))]
pub use link::{LinkState, X32Link};

#[cfg(not(target_arch = "wasm32"))]
mod link {
    use super::{Move, PORT, decode, encode, msg, read_info, read_move};
    use std::collections::VecDeque;
    use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
    use std::time::{Duration, Instant};

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How often `/xremote` is renewed (the console forgets a client after 10 s).
    const RENEW: Duration = Duration::from_secs(8);
    /// Moves waiting for the UI thread (a console flood drops the oldest).
    const MAX_QUEUE: usize = 4096;

    /// How the link to the console is.
    #[derive(Debug, Clone, Default, PartialEq)]
    pub struct LinkState {
        /// `X32 4.06`, once the console answered.
        pub console: Option<String>,
        /// When it was last heard from.
        pub last_heard: Option<Instant>,
        /// Why there is no link (an address that does not resolve, a socket refused).
        pub error: Option<String>,
        /// Moves received so far.
        pub moves: u64,
    }

    struct Inner {
        stop: AtomicBool,
        bus: AtomicU8,
        stereo: AtomicBool,
        moves: Mutex<VecDeque<Move>>,
        state: Mutex<LinkState>,
        waker: Mutex<Option<crate::hub::Waker>>,
    }

    /// A running link to a console (stopped on [`X32Link::stop`] or when dropped).
    pub struct X32Link {
        inner: Arc<Inner>,
        address: String,
    }

    impl Drop for X32Link {
        fn drop(&mut self) {
            self.stop();
        }
    }

    impl X32Link {
        /// Starts following the console at `address` (`host` or `host:port`), its `bus`'s sends
        /// (the odd bus of a `stereo` pair, with the sends' pans).
        pub fn start(address: &str, bus: u8, stereo: bool, waker: Option<crate::hub::Waker>) -> X32Link {
            let inner = Arc::new(Inner {
                stop: AtomicBool::new(false),
                bus: AtomicU8::new(bus),
                stereo: AtomicBool::new(stereo),
                moves: Mutex::new(VecDeque::new()),
                state: Mutex::new(LinkState::default()),
                waker: Mutex::new(waker),
            });
            let addr = address.trim().to_string();
            let me = Arc::clone(&inner);
            let a = addr.clone();
            if std::thread::Builder::new().name("freedawm-x32".into()).spawn(move || run(&me, &a)).is_err() {
                lock(&inner.state).error = Some("could not start the network thread".into());
            }
            X32Link { inner, address: addr }
        }

        pub fn address(&self) -> &str {
            &self.address
        }

        /// The controller from now on: `bus` (the odd bus of a `stereo` pair).
        pub fn set_bus(&self, bus: u8, stereo: bool) {
            self.inner.bus.store(bus, Ordering::Relaxed);
            self.inner.stereo.store(stereo, Ordering::Relaxed);
        }

        pub fn stop(&self) {
            self.inner.stop.store(true, Ordering::SeqCst);
        }

        /// The moves received since last asked, oldest first.
        pub fn drain(&self) -> Vec<Move> {
            lock(&self.inner.moves).drain(..).collect()
        }

        pub fn state(&self) -> LinkState {
            lock(&self.inner.state).clone()
        }
    }

    fn resolve(address: &str) -> Result<SocketAddr, String> {
        let a = if address.contains(':') { address.to_string() } else { format!("{address}:{PORT}") };
        a.to_socket_addrs().map_err(|e| format!("{address}: {e}"))?.find(SocketAddr::is_ipv4).ok_or_else(|| format!("{address}: no IPv4 address"))
    }

    fn run(inner: &Inner, address: &str) {
        let set_error = |e: String| lock(&inner.state).error = Some(e);
        let peer = match resolve(address) {
            Ok(p) => p,
            Err(e) => return set_error(e),
        };
        let sock = match UdpSocket::bind(("0.0.0.0", 0)).and_then(|s| s.connect(peer).map(|()| s)) {
            Ok(s) => s,
            Err(e) => return set_error(format!("the network refused: {e}")),
        };
        if let Err(e) = sock.set_read_timeout(Some(Duration::from_millis(100))) {
            return set_error(e.to_string());
        }
        lock(&inner.state).error = None;
        let mut renewed: Option<Instant> = None;
        let mut buf = vec![0u8; 8192];
        while !inner.stop.load(Ordering::SeqCst) {
            if renewed.is_none_or(|t| t.elapsed() >= RENEW) {
                renewed = Some(Instant::now());
                let _ = sock.send(&encode(&msg("/xremote")));
                let _ = sock.send(&encode(&msg("/info")));
            }
            let n = match sock.recv(&mut buf) {
                Ok(n) => n,
                // A timeout, or nothing listening there yet (ICMP refused): try again.
                Err(_) => continue,
            };
            let bus = inner.bus.load(Ordering::Relaxed);
            let stereo = inner.stereo.load(Ordering::Relaxed);
            let mut got = 0;
            {
                let mut st = lock(&inner.state);
                st.last_heard = Some(Instant::now());
                let mut q = lock(&inner.moves);
                for m in decode(buf.get(..n).unwrap_or_default()) {
                    if let Some(info) = read_info(&m) {
                        st.console = Some(info);
                    } else if let Some(mv) = read_move(&m, bus, stereo) {
                        if q.len() >= MAX_QUEUE {
                            q.pop_front();
                        }
                        q.push_back(mv);
                        got += 1;
                    }
                }
                st.moves += got;
            }
            if got > 0
                && let Some(w) = lock(&inner.waker).clone()
            {
                w();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc_messages_round_trip() {
        let m = Message { addr: "/ch/01/mix/16/level".into(), args: vec![Arg::Float(0.75)] };
        let b = encode(&m);
        assert_eq!(b.len() % 4, 0);
        assert_eq!(&b[..20], b"/ch/01/mix/16/level\0");
        assert_eq!(decode(&b), vec![m]);
        let info = Message {
            addr: "/info".into(),
            args: vec![Arg::Str("V2.07".into()), Arg::Str("osc-server".into()), Arg::Str("X32".into()), Arg::Str("4.06".into())],
        };
        assert_eq!(read_info(&decode(&encode(&info))[0]), Some("X32 4.06".into()));
        let q = msg("/xremote");
        assert_eq!(encode(&q), b"/xremote\0\0\0\0,\0\0\0");
        assert_eq!(decode(&encode(&q)), vec![q]);
    }

    #[test]
    fn a_bundle_holds_messages_and_garbage_is_dropped() {
        let a = encode(&Message { addr: "/bus/16/mix/fader".into(), args: vec![Arg::Float(0.5)] });
        let c = encode(&Message { addr: "/ch/02/mix/16/level".into(), args: vec![Arg::Float(1.0)] });
        let mut b = b"#bundle\0".to_vec();
        b.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
        for el in [&a, &c] {
            b.extend_from_slice(&(el.len() as u32).to_be_bytes());
            b.extend_from_slice(el);
        }
        assert_eq!(decode(&b).len(), 2);
        // Truncated, hostile, empty.
        assert!(decode(&a[..a.len() - 2]).is_empty());
        assert!(decode(b"").is_empty());
        assert!(decode(b"not osc at all").is_empty());
        let mut huge = b"#bundle\0\0\0\0\0\0\0\0\0".to_vec();
        huge.extend_from_slice(&u32::MAX.to_be_bytes());
        assert!(decode(&huge).is_empty());
    }

    #[test]
    fn the_fader_law_matches_the_console() {
        assert_eq!(fader_to_db(0.75), 0.0);
        assert_eq!(fader_to_db(1.0), 10.0);
        assert_eq!(fader_to_db(0.5), -10.0);
        assert_eq!(fader_to_db(0.25), -30.0);
        assert_eq!(fader_to_db(0.0625), -60.0);
        assert_eq!(fader_to_db(0.0), f32::NEG_INFINITY);
        for db in [-89.0f32, -60.0, -45.5, -30.0, -12.0, -10.0, -3.0, 0.0, 6.0, 10.0] {
            assert!((fader_to_db(db_to_fader(db)) - db).abs() < 1e-3, "{db}");
        }
        assert_eq!(db_to_fader(-200.0), 0.0);
        assert_eq!(db_to_fader(20.0), 1.0);
        assert_eq!(fader_to_db(f32::NAN), f32::NEG_INFINITY);
    }

    #[test]
    fn console_moves_move_the_sessions_tracks_as_one_undo_step() {
        use soundcraft_model::TrackKind;
        let mut e = soundcraft_engine::Engine::default();
        e.execute("track.new", &serde_json::json!({"count": 3, "format": "Mono"})).unwrap();
        e.execute("track.new", &serde_json::json!({"kind": "master"})).unwrap();
        let strips = strip_tracks(e.session());
        assert_eq!(strips.len(), 3);
        let steps = e.undo_history().len();
        let t0 = std::time::Instant::now();
        let mut a = Apply::default();
        // A drag on channel 2 (many messages), channel 9 that has no track, and the bus master.
        let moves = [
            Move::Strip { channel: 2, db: -3.0 },
            Move::Strip { channel: 2, db: -5.0 },
            Move::Strip { channel: 9, db: -5.0 },
            Move::Master { db: -2.0 },
        ];
        assert_eq!(a.apply(&mut e, &moves, true, t0), 2);
        assert_eq!(a.apply(&mut e, &[Move::Strip { channel: 2, db: -6.0 }], true, t0 + std::time::Duration::from_millis(300)), 1);
        let vol = |e: &soundcraft_engine::Engine, t| e.session().track(t).unwrap().mixer.volume_db;
        assert_eq!(vol(&e, strips[1]), -6.0);
        assert_eq!(vol(&e, strips[0]), 0.0);
        let master = e.session().tracks.iter().find(|t| t.kind == TrackKind::Master).unwrap().id;
        assert_eq!(vol(&e, master), -2.0);
        assert_eq!(e.undo_history().len(), steps + 1, "one gesture, one undo step");
        // A second later it is a new gesture.
        a.apply(&mut e, &[Move::Strip { channel: 1, db: -1.0 }], true, t0 + std::time::Duration::from_secs(3));
        assert_eq!(e.undo_history().len(), steps + 2);
        // The master off: the bus master moves nothing.
        assert_eq!(a.apply(&mut e, &[Move::Master { db: -20.0 }], false, t0 + std::time::Duration::from_secs(3)), 0);
        assert_eq!(vol(&e, master), -2.0);
    }

    #[test]
    fn only_the_controller_bus_moves_anything() {
        let level = |ch: u8, bus: u8, f: f32| Message { addr: format!("/ch/{ch:02}/mix/{bus:02}/level"), args: vec![Arg::Float(f)] };
        assert_eq!(read_move(&level(3, 16, 0.75), 16, false), Some(Move::Strip { channel: 3, db: 0.0 }));
        assert_eq!(read_move(&level(3, 15, 0.75), 16, false), None, "another bus is the console's own mix");
        assert_eq!(read_move(&level(3, 16, 0.0), 16, false), Some(Move::Strip { channel: 3, db: -144.0 }));
        // The channel's own fader is the live mix: never followed.
        let fader = Message { addr: "/ch/03/mix/fader".into(), args: vec![Arg::Float(0.75)] };
        assert_eq!(read_move(&fader, 16, false), None);
        let master = Message { addr: "/bus/16/mix/fader".into(), args: vec![Arg::Float(0.5)] };
        assert_eq!(read_move(&master, 16, false), Some(Move::Master { db: -10.0 }));
        assert_eq!(read_move(&master, 12, false), None);
        assert_eq!(read_move(&level(33, 16, 0.5), 16, false), None);
        // A stereo pair speaks through its odd bus, and its sends have a pan.
        let pan = |ch: u8, bus: u8, f: f32| Message { addr: format!("/ch/{ch:02}/mix/{bus:02}/pan"), args: vec![Arg::Float(f)] };
        assert_eq!(read_move(&pan(4, 15, 0.0), 15, true), Some(Move::Pan { channel: 4, pan: -1.0 }));
        assert_eq!(read_move(&pan(4, 15, 0.75), 15, true), Some(Move::Pan { channel: 4, pan: 0.5 }));
        assert_eq!(read_move(&pan(4, 15, 0.75), 15, false), None, "a mono bus has no pan");
        assert_eq!(read_move(&pan(4, 13, 0.75), 15, true), None);
    }

    #[test]
    fn a_stereo_pair_is_its_odd_bus() {
        let c = |bus: u8, stereo: bool| X32Config { bus, stereo, ..X32Config::default() };
        assert_eq!(c(16, true).osc_bus(), 15);
        assert_eq!(c(15, true).osc_bus(), 15);
        assert_eq!(c(16, false).osc_bus(), 16);
        assert_eq!(c(16, true).label(), "Buses 15–16");
        assert_eq!(c(7, false).label(), "Bus 7");
        assert_eq!(c(0, false).osc_bus(), 1);
    }

    #[test]
    fn a_send_pan_pans_mono_and_stereo_tracks() {
        let mut e = soundcraft_engine::Engine::default();
        e.execute("track.new", &serde_json::json!({"count": 1, "format": "Mono"})).unwrap();
        e.execute("track.new", &serde_json::json!({"count": 1, "format": "Stereo"})).unwrap();
        let strips = strip_tracks(e.session());
        let mut a = Apply::default();
        let now = std::time::Instant::now();
        a.apply(&mut e, &[Move::Pan { channel: 1, pan: -0.5 }, Move::Pan { channel: 2, pan: 0.25 }], true, now);
        let pans = |e: &soundcraft_engine::Engine, t| e.session().track(t).unwrap().mixer.pan.clone();
        assert_eq!(pans(&e, strips[0]), vec![-0.5]);
        let st = pans(&e, strips[1]);
        assert_eq!(st.len(), 2);
        assert!(((st[0] + st[1]) * 0.5 - 0.25).abs() < 1e-5, "{st:?}");
    }
}
