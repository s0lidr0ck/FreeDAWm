//! Remote mixing: another FreeDAWm PC mixes this session (spec
//! `docs/superpowers/specs/2026-11-09-remote-mixing-design.md`).
//!
//! The host (the PC running the session and the audio hardware) stays the source of truth. A
//! paired PC says `hello` with `role: "mixer"`; the host then pushes it its session in pieces
//! (each track, the track order and the rest of the document, only those that changed, then a
//! `mix.rev` that says the pieces make a whole), its meters and transport ~30 times a second,
//! and, when asked, its mix as the remote mix stream. The remote PC shows the host's session in
//! its own Mix window: a mix or automation change runs there at once (so the fader follows the
//! hand) and goes to the host, which merges a gesture into one undo step as a local drag does
//! and writes automation from it; transport commands run only at the host; anything that would
//! edit the session otherwise is refused on the remote PC ([`route`]). The host checks the same
//! list ([`host_allows`]).
//!
//! The pure parts (the routing rule, the pieces, the meter frame) build everywhere; the client
//! and the host's pushing are native only.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where a command runs while this PC mixes another remotely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Here only: a view, a selection, a query.
    Local,
    /// Here at once and at the host (mix and automation changes).
    Both,
    /// At the host only (the transport, undo and redo of the host's history).
    Remote,
    /// Not while mixing remotely (it would edit the session or replace it).
    Refuse,
}

/// The reason a refused command gives.
pub const REFUSED: &str = "while mixing another PC remotely, only mix, automation and transport changes are possible";

/// Where command `id` runs on a PC mixing another remotely.
pub fn route(id: &str) -> Route {
    if id.starts_with("transport.") || matches!(id, "edit.undo" | "edit.redo") {
        return Route::Remote;
    }
    let spec = soundcraft_engine::find_command(id);
    // Queries (not journalled, not undoable, not views) only read: they run here.
    let query = spec.is_some_and(|s| !s.journal && !s.undoable && !s.view);
    if id.starts_with("mix.") || id.starts_with("automation.") {
        return if query { Route::Local } else { Route::Both };
    }
    if id.starts_with("session.") || id.starts_with("file.") || id.starts_with("process.") {
        return Route::Refuse;
    }
    match spec {
        Some(s) if s.undoable => Route::Refuse,
        _ => Route::Local,
    }
}

/// What a mixer connection may run at the host: the commands a remote PC sends ([`route`]).
pub fn host_allows(id: &str) -> bool {
    matches!(route(soundcraft_engine::cmd::canonical_id(id)), Route::Both | Route::Remote)
}

/// One meter: a track's (by id) or the main's, linear peaks 0..1 (and more) per side, and gain
/// reduction in dB (0 when none).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Meter {
    pub id: u64,
    pub l: f32,
    pub r: f32,
    #[serde(default)]
    pub gr: f32,
}

/// The host's meters and transport, ~30 times a second (`mix.meters`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MeterFrame {
    pub tracks: Vec<Meter>,
    /// The main mix (its `id` is 0).
    pub main: Meter,
    pub playing: bool,
    #[serde(default)]
    pub recording: bool,
    /// The playhead, samples at the session rate.
    pub position: i64,
}

/// A session cut into the pieces the host sends: the document without its tracks, the track
/// order, and each track (all JSON text).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Pieces {
    pub rest: String,
    pub order: Vec<u64>,
    pub tracks: Vec<(u64, String)>,
}

/// Cuts `s` into its pieces (one serialization).
pub fn pieces(s: &soundcraft_model::Session) -> Result<Pieces, String> {
    let mut v = serde_json::to_value(s).map_err(|e| e.to_string())?;
    let tracks = v.get_mut("tracks").map(Value::take).unwrap_or(Value::Array(Vec::new()));
    if let Some(o) = v.as_object_mut() {
        o.insert("tracks".into(), Value::Array(Vec::new()));
    }
    let mut p = Pieces { rest: v.to_string(), ..Pieces::default() };
    for t in tracks.as_array().map(Vec::as_slice).unwrap_or_default() {
        let Some(id) = t.get("id").and_then(Value::as_u64) else { continue };
        p.order.push(id);
        p.tracks.push((id, t.to_string()));
    }
    Ok(p)
}

/// Puts the pieces together again: the document, tracks in `order`.
pub fn assemble(rest: &Value, order: &[u64], tracks: &std::collections::HashMap<u64, Value>) -> Result<soundcraft_model::Session, String> {
    let mut v = rest.clone();
    let list: Vec<Value> = order.iter().filter_map(|id| tracks.get(id).cloned()).collect();
    match v.as_object_mut() {
        Some(o) => {
            o.insert("tracks".into(), Value::Array(list));
        }
        None => return Err("the session from the host is not an object".into()),
    }
    serde_json::from_value(v).map_err(|e| format!("the session from the host: {e}"))
}

#[cfg(not(target_arch = "wasm32"))]
pub use client::{MixerClient, MixerTarget};

#[cfg(not(target_arch = "wasm32"))]
mod client {
    use super::{MeterFrame, assemble};
    use crate::hub::read_line;
    use serde_json::{Value, json};
    use std::collections::{HashMap, VecDeque};
    use std::io::{BufReader, Write};
    use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
    use std::time::{Duration, Instant};

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The host to mix, and as whom.
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub struct MixerTarget {
        pub host_name: String,
        /// `host:port`.
        pub addr: String,
        /// The token the host gave when this PC paired with it.
        pub token: String,
        /// This PC's name.
        pub my_name: String,
    }

    /// The largest line the host sends (a track with a lot of automation).
    const MAX_LINE: usize = 64 << 20;
    /// Meter frames kept for the delay that lines them up with the audio heard.
    const MAX_FRAMES: usize = 64;
    /// After the last answer to a command of ours, the host's document waits this long before it
    /// replaces ours: a piece the host sent before it ran our change must not pull a fader back.
    const HOLD: Duration = Duration::from_millis(120);

    #[derive(Default)]
    struct State {
        connected: bool,
        status: String,
        host_pc: String,
        rest: Option<Value>,
        order: Vec<u64>,
        tracks: HashMap<u64, Value>,
        /// The newest whole (`mix.rev`) and the one handed out.
        complete: u64,
        taken: u64,
        frames: VecDeque<(Instant, MeterFrame)>,
        /// Commands sent and not answered yet; when the last answer came.
        outstanding: usize,
        last_answer: Option<Instant>,
        errors: Vec<String>,
    }

    struct Inner {
        target: Mutex<MixerTarget>,
        state: Mutex<State>,
        out: Mutex<Option<Arc<Mutex<TcpStream>>>>,
        next: AtomicU64,
        stop: AtomicBool,
        want_stream: AtomicBool,
        stream_in: Mutex<Option<(Arc<soundcraft_playback::stream::StreamIn>, u32)>>,
        decoder: Mutex<Option<crate::stream::MixDecoder>>,
        waker: Mutex<Option<crate::hub::Waker>>,
    }

    /// A running link to the host being mixed (stopped when [`MixerClient::stop`] is called).
    #[derive(Clone)]
    pub struct MixerClient {
        inner: Arc<Inner>,
    }

    impl MixerClient {
        /// Connects to the host (and again whenever the link drops, until stopped).
        pub fn start(target: MixerTarget) -> MixerClient {
            let c = MixerClient {
                inner: Arc::new(Inner {
                    target: Mutex::new(target),
                    state: Mutex::new(State { status: "connecting".into(), ..State::default() }),
                    out: Mutex::new(None),
                    next: AtomicU64::new(1),
                    stop: AtomicBool::new(false),
                    want_stream: AtomicBool::new(false),
                    stream_in: Mutex::new(None),
                    decoder: Mutex::new(None),
                    waker: Mutex::new(None),
                }),
            };
            let me = c.clone();
            if std::thread::Builder::new().name("freedawm-mixer".into()).spawn(move || me.run()).is_err() {
                lock(&c.inner.state).status = "could not start the network thread".into();
            }
            c
        }

        pub fn stop(&self) {
            self.inner.stop.store(true, Ordering::SeqCst);
            if let Some(out) = lock(&self.inner.out).take() {
                let _ = lock(&out).shutdown(std::net::Shutdown::Both);
            }
            if let Some((q, _)) = &*lock(&self.inner.stream_in) {
                q.set_on(false);
            }
        }

        pub fn target(&self) -> MixerTarget {
            lock(&self.inner.target).clone()
        }

        /// Wakes the UI thread when something arrives.
        pub fn set_waker(&self, w: Option<crate::hub::Waker>) {
            *lock(&self.inner.waker) = w;
        }

        fn wake(&self) {
            if let Some(w) = lock(&self.inner.waker).clone() {
                w();
            }
        }

        pub fn connected(&self) -> bool {
            lock(&self.inner.state).connected
        }

        /// `connecting`, `connected`, `not paired`, `not reachable`…
        pub fn status(&self) -> String {
            lock(&self.inner.state).status.clone()
        }

        /// The host's PC name, once connected.
        pub fn host_pc(&self) -> String {
            lock(&self.inner.state).host_pc.clone()
        }

        /// Sends a command to the host (`merge`: the gesture it is part of). False when not
        /// connected.
        pub fn send_command(&self, id: &str, params: &Value, merge: Option<&str>) -> bool {
            let Some(out) = lock(&self.inner.out).clone() else { return false };
            let n = self.inner.next.fetch_add(1, Ordering::Relaxed);
            let mut p = json!({"command": id, "params": params});
            if let Some(k) = merge {
                p["merge"] = json!(format!("remote:{k}"));
            }
            lock(&self.inner.state).outstanding += 1;
            let ok = write(&out, &json!({"id": format!("c:{n}"), "method": "engine.execute", "params": p}));
            if !ok {
                let mut s = lock(&self.inner.state);
                s.outstanding = s.outstanding.saturating_sub(1);
            }
            ok
        }

        /// The host's session, when a newer whole has arrived and none of our changes is still on
        /// its way (so it never pulls back a fader being moved here).
        pub fn take_session(&self) -> Option<Result<soundcraft_model::Session, String>> {
            let mut s = lock(&self.inner.state);
            if s.complete <= s.taken || s.outstanding > 0 || s.last_answer.is_some_and(|t| t.elapsed() < HOLD) {
                return None;
            }
            s.taken = s.complete;
            let rest = s.rest.clone()?;
            Some(assemble(&rest, &s.order, &s.tracks))
        }

        /// Whether a whole session has arrived since connecting.
        pub fn has_session(&self) -> bool {
            lock(&self.inner.state).complete > 0
        }

        /// The newest meter frame at least `delay` old (the stream's delay, so the meters move
        /// with what is heard), else the oldest.
        pub fn meters(&self, delay: Duration) -> Option<MeterFrame> {
            let s = lock(&self.inner.state);
            let now = Instant::now();
            s.frames.iter().rev().find(|(t, _)| now.duration_since(*t) >= delay).or(s.frames.front()).map(|(_, f)| f.clone())
        }

        /// Errors the host answered our commands with, since last asked.
        pub fn take_errors(&self) -> Vec<String> {
            std::mem::take(&mut lock(&self.inner.state).errors)
        }

        /// Hear the host's mix (or not) through the player's stream input.
        pub fn set_stream(&self, want: bool, stream_in: Option<Arc<soundcraft_playback::stream::StreamIn>>, rate: u32) {
            let was = self.inner.want_stream.swap(want, Ordering::SeqCst);
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
            if let Some((q, _)) = &now {
                q.set_on(want && self.connected());
            }
            if was != want
                && let Some(out) = lock(&self.inner.out).clone()
            {
                write(&out, &json!({"method": "stream.subscribe", "params": {"on": want}}));
            }
        }

        /// The jitter buffer's delay (what the audio heard lags the host by), while hearing it.
        pub fn stream_delay(&self) -> Option<Duration> {
            if !self.inner.want_stream.load(Ordering::SeqCst) {
                return None;
            }
            let d = lock(&self.inner.decoder);
            d.as_ref().map(|d| Duration::from_secs_f64(d.target_ms.max(0.0) / 1000.0))
        }

        fn stopped(&self) -> bool {
            self.inner.stop.load(Ordering::SeqCst)
        }

        fn set_status(&self, f: impl FnOnce(&mut State)) {
            f(&mut lock(&self.inner.state));
            self.wake();
        }

        fn run(&self) {
            while !self.stopped() {
                let addr = lock(&self.inner.target).addr.to_socket_addrs().ok().and_then(|mut a| a.find(SocketAddr::is_ipv4));
                match addr {
                    None => self.set_status(|s| s.status = "no address for the host".into()),
                    Some(a) => {
                        if let Err(e) = self.session(a) {
                            log::debug!("mixer: {e}");
                            self.set_status(|s| {
                                if s.status != "not paired" {
                                    s.status = "not reachable".into();
                                }
                            });
                        }
                    }
                }
                *lock(&self.inner.out) = None;
                self.set_status(|s| {
                    s.connected = false;
                    s.outstanding = 0;
                });
                if let Some((q, _)) = &*lock(&self.inner.stream_in) {
                    q.set_on(false);
                }
                let end = Instant::now() + Duration::from_secs(2);
                while Instant::now() < end && !self.stopped() {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }

        fn session(&self, addr: SocketAddr) -> Result<(), String> {
            let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| e.to_string())?;
            let _ = stream.set_nodelay(true);
            stream.set_read_timeout(Some(Duration::from_secs(15))).map_err(|e| e.to_string())?;
            stream.set_write_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
            let out = Arc::new(Mutex::new(stream.try_clone().map_err(|e| e.to_string())?));
            let mut reader = BufReader::new(stream);
            let t = self.target();
            write(&out, &json!({"id": "hello", "method": "hello", "params": {"token": t.token, "name": t.my_name, "role": "mixer"}}));
            let mut buf = Vec::new();
            if !read_line(&mut reader, &mut buf, MAX_LINE)? {
                return Err("closed".into());
            }
            let hello: Value = serde_json::from_slice(&buf).map_err(|e| e.to_string())?;
            if hello["ok"] != true {
                let unpaired = hello["pair"] == true;
                self.set_status(|s| s.status = if unpaired { "not paired".into() } else { "refused".into() });
                return Err(hello["error"].as_str().unwrap_or("refused").to_string());
            }
            let pc = hello["result"]["pc"].as_str().unwrap_or("").to_string();
            self.set_status(|s| {
                s.connected = true;
                s.status = "connected".into();
                s.host_pc = pc;
                // A fresh whole comes on every connection.
                s.rest = None;
                s.order.clear();
                s.tracks.clear();
                s.frames.clear();
            });
            *lock(&self.inner.out) = Some(Arc::clone(&out));
            if self.inner.want_stream.load(Ordering::SeqCst) {
                write(&out, &json!({"method": "stream.subscribe", "params": {"on": true}}));
                if let Some((q, _)) = &*lock(&self.inner.stream_in) {
                    q.set_on(true);
                }
            }
            // The jitter buffer hands the decoded mix to the player every 10 ms.
            let done = Arc::new(AtomicBool::new(false));
            let pump = {
                let me = self.clone();
                let done = Arc::clone(&done);
                std::thread::Builder::new().name("freedawm-mixer-pump".into()).spawn(move || {
                    while !done.load(Ordering::SeqCst) && !me.stopped() {
                        me.pump_stream();
                        std::thread::sleep(Duration::from_millis(10));
                    }
                })
            };
            let r = loop {
                if self.stopped() {
                    break Ok(());
                }
                match read_line(&mut reader, &mut buf, MAX_LINE) {
                    Ok(true) => {}
                    Ok(false) => break Ok(()),
                    Err(e) => break Err(e),
                }
                let Ok(v) = serde_json::from_slice::<Value>(&buf) else { continue };
                self.on_line(&v);
            };
            done.store(true, Ordering::SeqCst);
            if let Ok(h) = pump {
                let _ = h.join();
            }
            r
        }

        fn pump_stream(&self) {
            let s = lock(&self.inner.stream_in).clone();
            let Some((queue, _)) = s else { return };
            if let Some(d) = lock(&self.inner.decoder).as_mut() {
                d.pump(&queue);
            }
        }

        fn on_line(&self, v: &Value) {
            let p = &v["params"];
            match v["event"].as_str() {
                Some("stream") => {
                    self.on_stream(p);
                    return;
                }
                Some("mix.rest") => {
                    lock(&self.inner.state).rest = Some(p["rest"].clone());
                    return;
                }
                Some("mix.order") => {
                    let order: Vec<u64> = p["order"].as_array().map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
                    let mut s = lock(&self.inner.state);
                    s.tracks.retain(|id, _| order.contains(id));
                    s.order = order;
                    return;
                }
                Some("mix.track") => {
                    if let Some(id) = p["id"].as_u64() {
                        lock(&self.inner.state).tracks.insert(id, p["track"].clone());
                    }
                    return;
                }
                Some("mix.rev") => {
                    let mut s = lock(&self.inner.state);
                    if s.rest.is_some() {
                        s.complete += 1;
                    }
                    drop(s);
                    self.wake();
                    return;
                }
                Some("mix.meters") => {
                    if let Ok(f) = serde_json::from_value::<MeterFrame>(p.clone()) {
                        let mut s = lock(&self.inner.state);
                        s.frames.push_back((Instant::now(), f));
                        while s.frames.len() > MAX_FRAMES {
                            s.frames.pop_front();
                        }
                        drop(s);
                        self.wake();
                    }
                    return;
                }
                Some(_) => return,
                None => {}
            }
            // An answer to a command of ours.
            if v["id"].as_str().is_some_and(|id| id.starts_with("c:")) {
                let mut s = lock(&self.inner.state);
                s.outstanding = s.outstanding.saturating_sub(1);
                s.last_answer = Some(Instant::now());
                if v["ok"] == false {
                    let e = v["error"].as_str().unwrap_or("refused").to_string();
                    if s.errors.len() < 20 {
                        s.errors.push(e);
                    }
                }
                drop(s);
                self.wake();
            }
        }

        fn on_stream(&self, p: &Value) {
            let Ok(packet) = serde_json::from_value::<crate::stream::StreamPacket>(p.clone()) else { return };
            let s = lock(&self.inner.stream_in).clone();
            let Some((queue, rate)) = s else { return };
            let mut d = lock(&self.inner.decoder);
            let dec = d.get_or_insert_with(|| crate::stream::MixDecoder::new(rate));
            dec.on_packet(packet, &queue);
        }
    }

    fn write(out: &Arc<Mutex<TcpStream>>, v: &Value) -> bool {
        let mut s = lock(out);
        writeln!(s, "{v}").is_ok() && s.flush().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_and_transport_go_to_the_host_and_editing_is_refused() {
        assert_eq!(route("mix.volume"), Route::Both);
        assert_eq!(route("mix.pan"), Route::Both);
        assert_eq!(route("mix.automation_mode"), Route::Both);
        assert_eq!(route("automation.touch"), Route::Both);
        assert_eq!(route("automation.touched"), Route::Local, "a query reads here");
        assert_eq!(route("transport.play"), Route::Remote);
        assert_eq!(route("edit.undo"), Route::Remote);
        assert_eq!(route("track.new"), Route::Refuse);
        assert_eq!(route("session.save"), Route::Refuse);
        assert_eq!(route("session.open"), Route::Refuse);
        assert!(host_allows("mix.volume") && host_allows("transport.stop"));
        assert!(!host_allows("track.delete") && !host_allows("session.save") && !host_allows("app.quit"));
    }

    #[test]
    fn a_session_cut_into_pieces_goes_back_together() {
        let mut e = soundcraft_engine::Engine::default();
        e.execute("track.new", &serde_json::json!({"count": 3, "format": "Mono"})).unwrap();
        let p = pieces(e.session()).unwrap();
        assert_eq!(p.order.len(), e.session().tracks.len());
        let rest: Value = serde_json::from_str(&p.rest).unwrap();
        let tracks = p.tracks.iter().map(|(id, t)| (*id, serde_json::from_str(t).unwrap())).collect();
        let back = assemble(&rest, &p.order, &tracks).unwrap();
        assert_eq!(pieces(&back).unwrap(), p);
    }
}
