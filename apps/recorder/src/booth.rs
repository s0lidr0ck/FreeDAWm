//! Following a FreeDAWm booth on another PC, as the DAW's remote recorder does, over the same
//! code (`soundcraft_net::follower` and `soundcraft_net::glue::FollowerGlue`): pairing with the
//! booth's 4-digit code, recording when it records, sending each take, hearing its mix.
//!
//! The recorder only ever dials: it starts no hub, announces nothing and accepts no connection.
//! Nothing goes on the network until the user asks: looking for booths (an mDNS query) when
//! **Find booths** is pressed, a pairing request when **Pair** is, and the link to the booth
//! once paired and following. Over the booth's encrypted local link the recorder dials from a UDP
//! socket of its own that answers no one, through a bridge on this PC only (127.0.0.1), exactly as
//! the DAW does. Joining by internet invite is not offered: the internet endpoint asks the router
//! for a port mapping and takes incoming connections, which the recorder never does.

use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_engine::midi_record::RecordedEvent;
use soundcraft_model::TrackId;
use soundcraft_net::config::NetConfig;
use soundcraft_net::discovery::{Discovery, Found};
use soundcraft_net::follower::{FollowTarget, Follower, StreamHealth, pair_confirm, pair_request};
use soundcraft_net::glue::{FinishedAudio, FinishedTake, FollowBadge, FollowerGlue, HostView, LiveAudio, RigHost};
use soundcraft_net::hub::TakeClock;
use soundcraft_net::pairing::exchange::LinkKeys;
use soundcraft_playback::TransportProbe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use crate::rig::Rig;
use crate::settings::{file_safe, now_stamp, recorder_name, unique_stem};

/// The take clock the follower reads: the player's frames rolled since Play, less the output
/// latency and mix delay (the audible position), in seconds (the DAW's `AppClock`).
#[derive(Default)]
pub struct RigClock {
    probe: RwLock<Option<TransportProbe>>,
    rate: AtomicU32,
    latency: AtomicI64,
}

impl RigClock {
    fn update(&self, rig: &Rig) {
        *self.probe.write().unwrap_or_else(PoisonError::into_inner) = Some(rig.player.probe());
        self.rate.store(rig.rate(), Ordering::Relaxed);
        self.latency.store(rig.midi_latency(), Ordering::Relaxed);
    }
}

impl TakeClock for RigClock {
    fn now(&self) -> Option<f64> {
        let probe = self.probe.read().unwrap_or_else(PoisonError::into_inner);
        let r = probe.as_ref()?.read();
        if !r.playing {
            return None;
        }
        let rate = f64::from(self.rate.load(Ordering::Relaxed).max(1));
        let since = soundcraft_playback::clock_ns().saturating_sub(r.at_ns) as f64 * 1e-9;
        let rolled = r.rolled as f64 + since.min(0.1) * rate;
        Some((rolled - self.latency.load(Ordering::Relaxed) as f64) / rate)
    }
}

/// The recorder as the follower glue sees it (the DAW's `AppHost`).
pub struct Host<'a> {
    pub rig: &'a mut Rig,
    /// Where the booth takes' own files go (the ideas folder).
    pub folder: PathBuf,
    /// The booth's session (the takes' names start with it).
    pub session: String,
    pub said: &'a mut Option<String>,
}

impl RigHost for Host<'_> {
    fn engine(&self) -> &Engine {
        &self.rig.engine
    }
    fn engine_mut(&mut self) -> &mut Engine {
        &mut self.rig.engine
    }
    fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        let from = self.rig.engine.session().edit.selection.start.max(0);
        match id {
            "transport.record" => {
                if self.rig.recording() {
                    return Ok(json!({}));
                }
                let prefix = if self.session.trim().is_empty() { "Booth".to_string() } else { file_safe(&self.session) };
                let stem = unique_stem(&self.folder, &prefix, now_stamp());
                self.rig.start(&self.folder, &stem, true, from)?;
                Ok(json!({}))
            }
            "transport.play" => {
                self.rig.player.play(from, None, None);
                Ok(json!({}))
            }
            "transport.stop" => {
                if self.rig.recording() {
                    let saved = self.rig.stop();
                    if let Some(p) = saved.problem {
                        *self.said = Some(p);
                    }
                } else {
                    self.rig.player.stop();
                }
                Ok(json!({}))
            }
            _ => {
                let v = self.rig.engine.execute(id, &params).map_err(|e| e.to_string())?;
                self.rig.player.update_session(self.rig.engine.session_arc());
                Ok(v)
            }
        }
    }
    fn view(&self) -> HostView {
        let from = self.rig.take_from();
        HostView {
            playing: self.rig.player.is_playing(),
            recording: self.rig.recording(),
            position: self.rig.player.position(),
            play_from: from,
            record_start: from,
            looped: None,
            punch: None,
            counting_off: false,
        }
    }
    fn level(&self, _track: TrackId) -> f32 {
        self.rig.level()
    }
    fn disk_free(&self) -> Option<u64> {
        soundcraft_net::sys::disk_free(&self.folder)
    }
    fn take_finished(&mut self) -> Option<FinishedTake> {
        let (take, events) = self.rig.booth_take.as_mut()?.midi.take()?;
        Some(FinishedTake { take, events })
    }
    fn live_since(&self, from: usize) -> Vec<RecordedEvent> {
        self.rig.live_since(from)
    }
    fn midi_ports(&self) -> Vec<String> {
        self.rig.midi_ports().into_iter().filter(|(_, on)| *on).map(|(n, _)| n).collect()
    }
    fn notify(&mut self, text: String) {
        *self.said = Some(text);
    }
    fn keep_take(&mut self, _booth_session: &str) -> Option<PathBuf> {
        // The take's own files are in the ideas folder already; the outbox keeps what is sent.
        None
    }
    fn audio_finished(&mut self) -> Option<FinishedAudio> {
        let (path, start, latency, rate, input) = self.rig.booth_take.as_mut()?.audio.take()?;
        Some(FinishedAudio { start, latency, rate, takes: vec![(self.rig.audio_track(), input, path)] })
    }
    fn audio_inputs(&self) -> Vec<String> {
        if self.rig.choice.on && self.rig.input_channels() > 0 { vec![self.rig.choice.name()] } else { Vec::new() }
    }
    fn live_audio(&self, from: &[(TrackId, usize)]) -> Vec<LiveAudio> {
        let track = self.rig.audio_track();
        let begin = from.iter().find(|(t, _)| *t == track).map_or(0, |(_, n)| *n);
        let Some((track, input, from, peaks, start)) = self.rig.live_audio(begin) else { return Vec::new() };
        vec![LiveAudio { track, input, from, peaks, block: soundcraft_playback::record::LIVE_BLOCK, start, latency: self.rig.record_offset() }]
    }
    fn project_folder(&self) -> Option<PathBuf> {
        None
    }
}

/// Pairing with a booth, on a thread.
#[derive(Debug, Clone, Default)]
struct PairJob {
    request: Option<String>,
    addr: String,
    result: Arc<Mutex<Option<Result<Value, String>>>>,
}

/// What the window shows about the booth.
#[derive(Debug, Clone, PartialEq)]
pub struct BoothView {
    pub badge: FollowBadge,
    pub connected: bool,
    /// Why not, while not connected ("connecting", "refused"...).
    pub state: String,
    pub link: &'static str,
    pub session: String,
    /// `Recording`, `Playing` or `Stopped`, and the position (s).
    pub transport: &'static str,
    pub position: f64,
    pub queued: usize,
    pub progress: Option<f64>,
    pub stream: Option<StreamHealth>,
}

/// The recorder's link to a booth.
pub struct Link {
    pub cfg: NetConfig,
    path: Option<PathBuf>,
    outbox: Option<PathBuf>,
    discovery: Option<Discovery>,
    lan_client: Option<soundcraft_net::internet::Internet>,
    pub follower: Option<Follower>,
    glue: FollowerGlue,
    clock: Arc<RigClock>,
    pair: PairJob,
    pair_lan: Option<(String, String)>,
    pub pair_status: String,
    pub addr_edit: String,
    pub code_edit: String,
    pub error: Option<String>,
    /// The last thing the glue said.
    pub said: Option<String>,
    /// Renders and tests: shown instead of the live link.
    pub demo: Option<BoothView>,
}

impl Link {
    /// The link with the settings in `path` (`recorder-network.json`) and the outbox in `outbox`.
    /// Opens nothing; follows at once only a booth paired before with Follow on.
    pub fn new(path: Option<PathBuf>, outbox: Option<PathBuf>) -> Link {
        let mut cfg = path.as_deref().map(NetConfig::load).unwrap_or_default();
        // Never a listener: whatever the file says.
        (cfg.enabled, cfg.internet_host) = (false, false);
        if cfg.pc_name.trim().is_empty() {
            cfg.pc_name = recorder_name(&soundcraft_net::hub::computer_name());
        }
        let mut l = Link {
            addr_edit: cfg.follow.booth_addr.clone(),
            cfg,
            path,
            outbox,
            discovery: None,
            lan_client: None,
            follower: None,
            glue: FollowerGlue::default(),
            clock: Arc::new(RigClock::default()),
            pair: PairJob::default(),
            pair_lan: None,
            pair_status: String::new(),
            code_edit: String::new(),
            error: None,
            said: None,
            demo: None,
        };
        l.save();
        if l.cfg.follow.enabled && l.paired() {
            l.start_following();
        }
        l
    }

    fn save(&mut self) {
        if let Some(p) = &self.path
            && let Err(e) = self.cfg.save(p)
        {
            self.error = Some(format!("Network settings not saved: {e}"));
        }
    }

    fn update(&mut self, f: impl FnOnce(&mut NetConfig)) {
        f(&mut self.cfg);
        self.save();
    }

    /// This recorder's name on the booth.
    pub fn my_name(&self) -> String {
        self.cfg.pc_name.clone()
    }

    pub fn paired(&self) -> bool {
        !self.cfg.follow.token.is_empty()
    }

    /// Looks for booths on this network (an mDNS query, on request only).
    pub fn find(&mut self) {
        if self.discovery.is_none() {
            match Discovery::new() {
                Ok(d) => self.discovery = Some(d),
                Err(e) => {
                    self.error = Some(format!("Cannot look for booths: {e}"));
                    return;
                }
            }
        }
        if let Some(d) = &mut self.discovery
            && let Err(e) = d.browse()
        {
            self.error = Some(format!("Cannot look for booths: {e}"));
        }
    }

    /// The booths found so far (not other recorders, not this PC).
    pub fn found(&self) -> Vec<Found> {
        let me = self.my_name();
        self.discovery.as_ref().map(Discovery::found).unwrap_or_default().into_iter().filter(|f| f.name != me && f.role != "recorder").collect()
    }

    /// Picks a booth found (or typed: `name` empty, `addr` given).
    pub fn choose(&mut self, name: &str, addr: &str) {
        let (name, addr) = (name.to_string(), addr.trim().to_string());
        self.addr_edit = addr.clone();
        self.update(|c| {
            if c.follow.booth_name != name || c.follow.booth_addr != addr {
                c.follow.token.clear();
                c.follow.booth_key.clear();
                c.follow.booth_lan.clear();
            }
            c.follow.booth_name = name;
            c.follow.booth_addr = addr;
            c.follow.internet.clear();
        });
    }

    /// The endpoint this PC dials booths' encrypted local links from (a free UDP port; it
    /// serves nothing).
    fn lan_client(&mut self) -> Result<&soundcraft_net::internet::Internet, String> {
        if self.lan_client.is_none() {
            use soundcraft_net::internet::{Internet, key_from_hex, key_hex, new_key};
            let key = match key_from_hex(&self.cfg.internet_key) {
                Some(k) => k,
                None => {
                    let k = new_key();
                    self.update(|c| c.internet_key = key_hex(&k));
                    k
                }
            };
            self.lan_client = Some(Internet::start_lan(key, 0)?);
        }
        self.lan_client.as_ref().ok_or_else(|| "no encrypted link".to_string())
    }

    /// A port on this PC only, bridged to the booth's encrypted link at `addr` with key `key`.
    fn lan_bridge(&mut self, addr: &str, key: &str) -> Result<String, String> {
        let sock: std::net::SocketAddr = addr.parse().map_err(|_| format!("the booth's encrypted link address `{addr}` is not one"))?;
        let host = soundcraft_net::internet::lan_addr(key, sock).ok_or("the booth's key is damaged: pair again")?;
        Ok(self.lan_client()?.bridge_to(host)?.to_string())
    }

    /// Asks the booth to pair: it shows a 4-digit code.
    pub fn pair(&mut self) {
        let addr = self.cfg.follow.booth_addr.clone();
        if addr.is_empty() {
            self.pair_status = "Pick a booth, or type its address, first.".into();
            return;
        }
        *self.pair.result.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.pair.request = None;
        self.pair_status = "Asking the booth…".into();
        let lan = self.found().into_iter().find(|f| f.name == self.cfg.follow.booth_name).and_then(|f| f.lan);
        self.pair_lan = None;
        let addr = match lan {
            Some((a, k)) => match self.lan_bridge(&a, &k) {
                Ok(bridge) => {
                    self.pair_lan = Some((a, k));
                    bridge
                }
                Err(e) => {
                    log::info!("recorder: pairing over the plain link: {e}");
                    addr
                }
            },
            None => addr,
        };
        self.pair.addr = addr.clone();
        let (job, me) = (Arc::clone(&self.pair.result), self.my_name());
        std::thread::spawn(move || {
            let r = pair_request(&addr, &me).map(|id| json!({"request": id}));
            *job.lock().unwrap_or_else(PoisonError::into_inner) = Some(r);
        });
    }

    /// Waiting for the code the booth shows.
    pub fn wants_code(&self) -> bool {
        self.pair.request.is_some()
    }

    /// The code typed: proves it with the booth (it is never sent).
    pub fn confirm(&mut self) {
        let Some(req) = self.pair.request.clone() else { return };
        let (addr, code) = (self.pair.addr.clone(), std::mem::take(&mut self.code_edit));
        let keys = match self.pair_lan.clone() {
            Some((_, booth)) => self.lan_client().ok().map(|me| me.key_hex()).and_then(|me| LinkKeys::from_hex(&booth, &me)),
            None => Some(LinkKeys::PLAIN),
        };
        let Some(keys) = keys else {
            self.pair_status = "The booth's key is damaged: find the booth again and pair.".into();
            return;
        };
        self.pair_status = "Checking the code…".into();
        let job = Arc::clone(&self.pair.result);
        std::thread::spawn(move || {
            let r = pair_confirm(&addr, &req, code, keys).map(|(token, pc)| json!({"token": token, "pc": pc}));
            *job.lock().unwrap_or_else(PoisonError::into_inner) = Some(r);
        });
    }

    /// Forgets the booth's pairing (this side) and stops following it.
    pub fn forget(&mut self) {
        self.stop_following();
        self.update(|c| {
            c.follow.token.clear();
            c.follow.booth_key.clear();
            c.follow.booth_lan.clear();
            c.follow.enabled = false;
        });
        self.pair_status.clear();
    }

    fn pair_progress(&mut self) {
        let got = self.pair.result.lock().unwrap_or_else(PoisonError::into_inner).take();
        let Some(r) = got else { return };
        match r {
            Ok(v) if v.get("request").is_some() => {
                self.pair.request = v["request"].as_str().map(str::to_string);
                self.pair_status = "Type the code the booth shows:".into();
            }
            Ok(v) => {
                let token = v["token"].as_str().unwrap_or("").to_string();
                let booth = v["pc"].as_str().unwrap_or("").to_string();
                let lan = self.pair_lan.take();
                self.update(|c| {
                    c.follow.token = token;
                    if !booth.is_empty() {
                        c.follow.booth_name = booth;
                    }
                    (c.follow.booth_lan, c.follow.booth_key) = lan.unwrap_or_default();
                    c.follow.enabled = true;
                });
                self.pair.request = None;
                self.pair_status = "Paired.".into();
                self.start_following();
            }
            Err(e) => {
                self.pair.request = None;
                self.pair_status = e;
            }
        }
    }

    /// Follow the booth paired with (or stop).
    pub fn set_follow(&mut self, on: bool) {
        self.update(|c| c.follow.enabled = on);
        if on { self.start_following() } else { self.stop_following() }
    }

    fn start_following(&mut self) {
        self.stop_following();
        if !self.paired() {
            return;
        }
        let cfg = self.cfg.clone();
        let secure = !cfg.follow.booth_key.is_empty() && !cfg.follow.booth_lan.is_empty();
        let addr = if secure {
            // The booth's encrypted link where it announces it now (it may have moved), else where
            // it was paired; always to the key pinned then.
            let lan = self
                .found()
                .into_iter()
                .find(|f| f.name == cfg.follow.booth_name)
                .and_then(|f| f.lan)
                .filter(|(_, k)| *k == cfg.follow.booth_key)
                .map_or_else(|| cfg.follow.booth_lan.clone(), |(a, _)| a);
            if lan != cfg.follow.booth_lan {
                self.update(|c| c.follow.booth_lan = lan.clone());
            }
            match self.lan_bridge(&lan, &cfg.follow.booth_key) {
                Ok(a) => a,
                Err(e) => {
                    self.error = Some(e);
                    return;
                }
            }
        } else {
            cfg.follow.booth_addr.clone()
        };
        let target = FollowTarget { booth_name: cfg.follow.booth_name.clone(), addr, token: cfg.follow.token.clone(), my_name: self.my_name() };
        let f = Follower::start(target, self.outbox.clone(), Some(self.clock.clone() as Arc<dyn TakeClock>));
        if !secure && let Some(d) = &self.discovery {
            f.set_lookup(d.found_handle());
        }
        self.error = None;
        self.follower = Some(f);
    }

    fn stop_following(&mut self) {
        if let Some(f) = self.follower.take() {
            f.stop();
        }
    }

    /// Once a frame: pairing, the booth's transport, the takes, and the mix into the output when
    /// `listen` (at `gain`).
    pub fn step(&mut self, rig: &mut Rig, folder: &Path, listen: bool, gain: f32) {
        self.pair_progress();
        let Some(f) = self.follower.clone() else { return };
        self.clock.update(rig);
        let input = rig.player.stream_in();
        input.set_gain(gain);
        f.set_stream(listen, Some(input), rig.rate());
        let timeout = Duration::from_secs(self.cfg.contact_timeout_s.max(1));
        let mut said = None;
        let session = self.glue.booth_session().to_string();
        let mut host = Host { rig, folder: folder.to_path_buf(), session, said: &mut said };
        self.glue.step(&mut host, &f, timeout);
        if said.is_some() {
            self.said = said;
        }
    }

    /// What the window shows (`None`: not following).
    pub fn view(&self, rig: &Rig) -> Option<BoothView> {
        if let Some(d) = &self.demo {
            return Some(d.clone());
        }
        let f = self.follower.as_ref()?;
        let recording = self.glue.recording() && rig.recording();
        let transport = if recording {
            "Recording"
        } else if rig.player.is_playing() {
            "Playing"
        } else {
            "Stopped"
        };
        let secure = !self.cfg.follow.booth_key.is_empty() && !self.cfg.follow.booth_lan.is_empty();
        Some(BoothView {
            badge: self.glue.badge(f),
            connected: f.connected(),
            state: f.state(),
            link: if secure { "encrypted local link" } else { "local link, not encrypted" },
            session: self.glue.booth_session().to_string(),
            transport,
            position: rig.player.position() as f64 / f64::from(rig.rate().max(1)),
            queued: f.queued(),
            progress: f.progress(),
            stream: f.stream_health(),
        })
    }

    /// Stops following (the window closes): the outbox keeps what was not sent.
    pub fn shutdown(&mut self) {
        self.stop_following();
    }
}
