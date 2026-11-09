//! The whole rig in one process: a booth (engine + hub) and a piano PC (engine + follower)
//! talking over loopback TCP, the service remote as a fake web client, simulated audio clocks
//! (the piano PC's runs fast) and a cable that can be pulled (a proxy between them).
#![cfg(not(target_arch = "wasm32"))]

use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_engine::midi_record::{MidiTake, RecordedEvent, record_live_midi};
use soundcraft_net::config::NetConfig;
use soundcraft_net::follower::{FollowTarget, Follower};
use soundcraft_net::glue::{BoothGlue, FinishedAudio, FinishedTake, FollowerGlue, HostView, LiveAudio, RigHost};
use soundcraft_net::hub::{Hub, TakeClock, http_request};
use soundcraft_time::{Range, Samples};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SR: f64 = 48_000.0;

/// An audio device's clock: frames rolled since Play at `rate` × real time.
struct SimClock {
    rate: f64,
    started: Mutex<Option<Instant>>,
}

impl SimClock {
    fn new(rate: f64) -> Arc<SimClock> {
        Arc::new(SimClock { rate, started: Mutex::new(None) })
    }
    fn start(&self) {
        *self.started.lock().unwrap() = Some(Instant::now());
    }
    fn stop(&self) {
        *self.started.lock().unwrap() = None;
    }
}

impl TakeClock for SimClock {
    fn now(&self) -> Option<f64> {
        self.started.lock().unwrap().map(|t| t.elapsed().as_secs_f64() * self.rate)
    }
}

/// A host: an engine with a simulated transport (what the app's frame does with transport
/// requests), and a "pianist" whose notes are heard at the transport's position.
struct SimHost {
    e: Engine,
    clock: Arc<SimClock>,
    view: HostView,
    live: Vec<RecordedEvent>,
    finished: Option<FinishedTake>,
    said: Vec<String>,
    /// Options › Count Off: a Record counts off this long before the transport rolls (as the
    /// app's player does: recording and playing at once, the clock still, `counting_off`).
    countoff: Option<Duration>,
    counting_since: Option<Instant>,
    /// The project folder a kept take goes to (the app saves its session there).
    project: Option<std::path::PathBuf>,
    /// A guitarist on this PC's armed audio track: where (local timeline) each pluck was played,
    /// and the take written at Stop.
    plucks: Vec<Samples>,
    audio_done: Option<FinishedAudio>,
    takes_dir: Option<std::path::PathBuf>,
    /// Plucks this loud clip.
    pluck_level: f32,
    /// The record offset the recorder plans (frames): the mix's plugin delay here.
    rec_latency: Samples,
}

impl SimHost {
    fn new(clock: Arc<SimClock>) -> SimHost {
        SimHost {
            e: Engine::default(),
            clock,
            view: HostView::default(),
            live: Vec::new(),
            finished: None,
            said: Vec::new(),
            countoff: None,
            counting_since: None,
            project: None,
            plucks: Vec::new(),
            audio_done: None,
            takes_dir: None,
            pluck_level: 0.9,
            rec_latency: 0,
        }
    }

    /// The guitarist plucks now.
    fn pluck(&mut self) {
        self.tick();
        if self.view.recording {
            self.plucks.push(self.view.position);
        }
    }

    fn tick(&mut self) {
        if let (Some(since), Some(d)) = (self.counting_since, self.countoff)
            && since.elapsed() >= d
        {
            // The count-off is over: the transport rolls from the record point.
            self.counting_since = None;
            self.view.counting_off = false;
            self.clock.start();
        }
        if let Some(t) = self.clock.now() {
            self.view.position = self.view.play_from + (t * SR) as Samples;
        }
    }

    fn start(&mut self, record: bool, countoff: bool) -> Result<Value, String> {
        let sel = self.e.session().edit.selection;
        let looped = (record && self.e.session().edit.loop_record && !sel.is_empty()).then_some(sel);
        let counting = record && countoff && self.countoff.is_some();
        self.view = HostView {
            playing: true,
            recording: record,
            position: sel.start,
            play_from: sel.start,
            record_start: sel.start,
            looped,
            punch: None,
            counting_off: counting,
        };
        if counting {
            self.counting_since = Some(Instant::now());
        } else {
            self.clock.start();
        }
        self.live.clear();
        Ok(json!({}))
    }

    /// The pianist plays `msg` now.
    fn play(&mut self, msg: [u8; 3]) {
        self.tick();
        if self.view.recording {
            self.live.push(RecordedEvent { at: self.view.position, port: "Keys".into(), msg, pass: 0 });
        }
    }
}

impl RigHost for SimHost {
    fn engine(&self) -> &Engine {
        &self.e
    }
    fn engine_mut(&mut self) -> &mut Engine {
        &mut self.e
    }
    fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        self.tick();
        match id {
            "transport.record" => {
                if !self.e.session().tracks.iter().any(|t| t.mixer.record_arm) {
                    return Err("no track is record-enabled".into());
                }
                self.start(true, params.get("countoff").and_then(Value::as_bool).unwrap_or(true))
            }
            "transport.play" => self.start(false, false),
            "transport.stop" => {
                if self.view.recording
                    && let Some(dir) = &self.takes_dir
                    && let Some(t) = self.e.session().tracks.iter().find(|t| t.mixer.record_arm && t.kind == soundcraft_model::TrackKind::Audio)
                {
                    // The take as the recorder writes it: frame k at timeline record_start + k.
                    let len = (self.view.position - self.view.record_start).max(1) as usize;
                    let mut c = vec![0f32; len];
                    for (i, p) in self.plucks.iter().enumerate() {
                        let k = (p - self.view.record_start) as usize;
                        if let Some(x) = c.get_mut(k) {
                            *x = if i % 2 == 0 { self.pluck_level } else { -self.pluck_level };
                        }
                    }
                    let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![c] };
                    let opts = soundcraft_audio_io::EncodeOptions { bit_depth: soundcraft_audio_io::BitDepth::Float32, ..Default::default() };
                    std::fs::create_dir_all(dir).unwrap();
                    let path = dir.join(format!("Guitar_{:02}.wav", self.e.session().sources.len() + 1 + self.plucks.len()));
                    std::fs::write(&path, soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();
                    self.audio_done = Some(FinishedAudio {
                        start: self.view.record_start,
                        latency: self.rec_latency,
                        rate: 48_000,
                        takes: vec![(t.id, "In 1".into(), path)],
                    });
                    self.plucks.clear();
                }
                if self.view.recording {
                    let take = MidiTake { start: self.view.record_start, end: self.view.position, looped: self.view.looped, punch: None, latency: 0 };
                    // The local copy, as the app records it.
                    record_live_midi(&mut self.e, &take, &self.live);
                    self.finished = Some(FinishedTake { take, events: self.live.clone() });
                }
                self.view.playing = false;
                self.view.recording = false;
                self.view.counting_off = false;
                self.counting_since = None;
                self.clock.stop();
                Ok(json!({}))
            }
            _ => self.e.execute(id, &params).map_err(|e| e.to_string()),
        }
    }
    fn view(&self) -> HostView {
        self.view
    }
    fn take_finished(&mut self) -> Option<FinishedTake> {
        self.finished.take()
    }
    fn live_since(&self, from: usize) -> Vec<RecordedEvent> {
        self.live.get(from..).map(<[RecordedEvent]>::to_vec).unwrap_or_default()
    }
    fn midi_ports(&self) -> Vec<String> {
        vec!["Keys".into()]
    }
    fn notify(&mut self, text: String) {
        self.said.push(text);
    }
    fn keep_take(&mut self, _booth_session: &str) -> Option<std::path::PathBuf> {
        self.project.clone()
    }
    fn audio_finished(&mut self) -> Option<FinishedAudio> {
        self.audio_done.take()
    }
    fn live_audio(&self, from: &[(soundcraft_model::TrackId, usize)]) -> Vec<LiveAudio> {
        // The level so far in blocks of 480 frames (10 ms): the plucks' blocks are loud.
        let Some(t) = self.e.session().tracks.iter().find(|t| t.mixer.record_arm && t.kind == soundcraft_model::TrackKind::Audio) else {
            return Vec::new();
        };
        if !self.view.recording || self.takes_dir.is_none() {
            return Vec::new();
        }
        let block = 480usize;
        let have = ((self.view.position - self.view.record_start).max(0) as usize) / block;
        let start = from.iter().find(|(id, _)| *id == t.id).map_or(0, |(_, n)| *n);
        let peaks = (start..have)
            .map(|b| {
                let hit = self.plucks.iter().any(|p| ((p - self.view.record_start) as usize) / block == b);
                if hit { self.pluck_level } else { 0.01 }
            })
            .collect();
        vec![LiveAudio { track: t.id, input: "In 1".into(), from: start, peaks, block, start: self.view.record_start, latency: 0 }]
    }
    fn project_folder(&self) -> Option<std::path::PathBuf> {
        self.project.clone()
    }
}

/// A cable that can be pulled: forwards connections to `to` while up; pulling it cuts them all
/// and refuses new ones.
struct Cable {
    addr: SocketAddr,
    up: Arc<AtomicBool>,
    conns: Arc<Mutex<Vec<TcpStream>>>,
}

impl Cable {
    fn new(to: SocketAddr) -> Cable {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let up = Arc::new(AtomicBool::new(true));
        let conns: Arc<Mutex<Vec<TcpStream>>> = Arc::new(Mutex::new(Vec::new()));
        let (u, c) = (Arc::clone(&up), Arc::clone(&conns));
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                if !u.load(Ordering::SeqCst) {
                    let _ = s.shutdown(std::net::Shutdown::Both);
                    continue;
                }
                let Ok(b) = TcpStream::connect(to) else { continue };
                let _ = (s.set_nodelay(true), b.set_nodelay(true));
                c.lock().unwrap().extend([s.try_clone().unwrap(), b.try_clone().unwrap()]);
                for (mut from, mut into) in [(s.try_clone().unwrap(), b.try_clone().unwrap()), (b, s)] {
                    std::thread::spawn(move || {
                        let mut buf = [0u8; 8192];
                        while let Ok(n) = from.read(&mut buf) {
                            if n == 0 || into.write_all(&buf[..n]).is_err() {
                                break;
                            }
                        }
                        let _ = into.shutdown(std::net::Shutdown::Both);
                    });
                }
            }
        });
        Cable { addr, up, conns }
    }

    fn pull(&self) {
        self.up.store(false, Ordering::SeqCst);
        for s in self.conns.lock().unwrap().drain(..) {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    }

    fn plug(&self) {
        self.up.store(true, Ordering::SeqCst);
    }
}

struct Rig {
    hub: Hub,
    addr: SocketAddr,
    booth: SimHost,
    bglue: BoothGlue,
    piano: SimHost,
    pglue: FollowerGlue,
    follower: Follower,
    cable: Cable,
    phone: String,
    timeout: Duration,
}

const PIANO_PORT: &str = "Remote: Piano PC › Keys";

fn pair(hub: &Hub, addr: SocketAddr, name: &str) -> String {
    let (_, r) = http_request(addr, "POST", "/api/pair/request", None, Some(&json!({"name": name}))).unwrap();
    let id = r["request"].as_str().unwrap().to_string();
    let code = hub.pending_pairings().into_iter().find(|p| p.id == id).unwrap().code;
    let (_, r) = http_request(addr, "POST", "/api/pair/confirm", None, Some(&json!({"request": id, "code": code}))).unwrap();
    r["token"].as_str().unwrap().to_string()
}

impl Rig {
    /// A booth with an armed vocal track and a track on the piano PC's port; a piano PC with
    /// nothing set up (it arms a Piano track itself) whose clock runs `ppm` fast.
    fn new(ppm: f64, outbox: Option<std::path::PathBuf>) -> Rig {
        let hub = Hub::new(NetConfig { pc_name: "Booth".into(), ..NetConfig::default() }, None);
        let port = hub.listen(0).unwrap();
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let booth_clock = SimClock::new(1.0);
        hub.set_clock(Some(booth_clock.clone()));
        let mut booth = SimHost::new(booth_clock);
        let vox = booth.e.execute("track.new", &json!({"kind": "audio", "name": "Vocal 1"})).unwrap()["tracks"][0].as_u64().unwrap();
        booth.e.execute("mix.record_arm", &json!({"tracks": [vox], "value": true})).unwrap();
        let pm = booth.e.execute("track.new", &json!({"kind": "instrument", "name": "Piano MIDI"})).unwrap()["tracks"][0].as_u64().unwrap();
        booth.e.execute("track.input", &json!({"tracks": [pm], "input": format!("midi:{PIANO_PORT}")})).unwrap();
        let phone = pair(&hub, addr, "Phone");
        let piano_token = pair(&hub, addr, "Piano PC");
        let cable = Cable::new(addr);
        let piano_clock = SimClock::new(1.0 + ppm * 1e-6);
        let piano = SimHost::new(piano_clock.clone());
        let target = FollowTarget { booth_name: String::new(), addr: cable.addr.to_string(), token: piano_token, my_name: "Piano PC".into() };
        let follower = Follower::start(target, outbox, Some(piano_clock));
        follower.set_ping_every(Duration::from_millis(150));
        Rig {
            hub,
            addr,
            booth,
            bglue: BoothGlue::default(),
            piano,
            pglue: FollowerGlue::default(),
            follower,
            cable,
            phone,
            timeout: Duration::from_secs(7200),
        }
    }

    /// One frame on both PCs.
    fn frame(&mut self) {
        self.booth.tick();
        self.piano.tick();
        let control = self.bglue.step(&mut self.booth, &self.hub);
        assert!(control.is_empty());
        self.pglue.step(&mut self.piano, &self.follower, self.timeout);
    }

    /// Frames for `d`.
    fn run(&mut self, d: Duration) {
        let end = Instant::now() + d;
        while Instant::now() < end {
            self.frame();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn run_until(&mut self, what: &str, d: Duration, mut done: impl FnMut(&Rig) -> bool) {
        let end = Instant::now() + d;
        while !done(self) {
            assert!(Instant::now() < end, "timed out: {what}; booth said {:?}, piano said {:?}", self.booth.said, self.piano.said);
            self.frame();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// The service remote's button (from another thread, as a browser would, while frames run).
    fn press(&mut self, action: Value) -> (u16, Value) {
        let (addr, token) = (self.addr, self.phone.clone());
        let h = std::thread::spawn(move || http_request(addr, "POST", "/api/action", Some(&token), Some(&action)).unwrap());
        while !h.is_finished() {
            self.frame();
            std::thread::sleep(Duration::from_millis(5));
        }
        h.join().unwrap()
    }

    /// The pianist plays a note now; returns where the booth's timeline is at this instant.
    fn note(&mut self, pitch: u8) -> Samples {
        self.piano.play([0x90, pitch, 100]);
        self.booth.tick();
        let at = self.booth.view.position;
        std::thread::sleep(Duration::from_millis(40));
        self.piano.play([0x80, pitch, 0]);
        at
    }

    /// The landed notes on the booth's piano track: (pitch, timeline sample).
    fn landed(&self) -> Vec<(u8, Samples)> {
        let s = self.booth.e.session();
        let t = s.tracks.iter().find(|t| t.name == "Piano MIDI").unwrap();
        let mut v = Vec::new();
        for c in t.clips() {
            for n in &c.midi_playing().unwrap().notes {
                let tick = s.tempo.samples_to_ticks(c.start, s.sample_rate) + n.start;
                v.push((n.pitch, s.tempo.tick_to_samples(tick, s.sample_rate)));
            }
        }
        v.sort();
        v
    }
}

/// Notes within this of where the booth was when they were played (2 ms on a wired LAN, plus a
/// tick's rounding).
const TOLERANCE: Samples = 2 * 48 + 30;

fn assert_lands(rig: &Rig, played: &[(u8, Samples)]) {
    let landed = rig.landed();
    assert_eq!(landed.len(), played.len(), "landed {landed:?}, played {played:?}");
    for ((p, want), (q, got)) in played.iter().zip(&landed) {
        assert_eq!(p, q);
        eprintln!("note {p}: {} samples off", got - want);
        assert!((got - want).abs() <= TOLERANCE, "note {p}: landed at {got}, played at {want} ({} samples off)", got - want);
    }
}

#[test]
fn record_on_the_page_starts_the_booth_and_the_piano_pc_and_the_take_lands_aligned() {
    // 2000 ppm: the drift of 20 s at 100 ppm, within this test's few seconds.
    let mut rig = Rig::new(2000.0, None);
    rig.run_until("the piano PC connects", Duration::from_secs(10), |r| r.follower.connected());
    // Start the booth half a second in, so the PCs' positions mean something.
    rig.booth.e.session_mut().edit.selection = Range::point(24_000);
    let (code, r) = rig.press(json!({"action": "record"}));
    assert_eq!(code, 200, "{r}");
    assert!(rig.booth.view.recording, "the booth records");
    rig.run_until("the piano PC records", Duration::from_secs(5), |r| r.piano.view.recording);
    assert!(rig.piano.said.iter().any(|s| s.contains("Recording with")), "{:?}", rig.piano.said);
    // At the booth's position (seconds on the booth timeline): within a few frames.
    rig.booth.tick();
    rig.piano.tick();
    let gap = (rig.piano.view.position - rig.booth.view.position).abs();
    assert!(gap < 4_800, "the piano PC started {gap} samples away from the booth");
    // The piano PC armed a Piano track of its own.
    assert!(rig.piano.e.session().tracks.iter().any(|t| t.name == "Piano" && t.mixer.record_arm));
    let mut played = Vec::new();
    for (i, pitch) in [60u8, 64, 67, 72, 76].into_iter().enumerate() {
        rig.run(Duration::from_millis(300 + 70 * i as u64));
        played.push((pitch, rig.note(pitch)));
    }
    rig.run(Duration::from_millis(400));
    // The live sketch reached the booth.
    let sk = rig.hub.sketches();
    assert_eq!(sk.first().map(|s| s.port.as_str()), Some(PIANO_PORT));
    assert!(sk[0].notes.iter().filter(|n| n.vel > 0).count() >= 4, "{:?}", sk[0].notes);
    assert!(rig.hub.activity(PIANO_PORT).is_some());
    // A tap on Stop while recording does nothing; a hold stops.
    let (code, r) = rig.press(json!({"action": "stop"}));
    assert_eq!(code, 409, "{r}");
    assert!(rig.booth.view.recording, "a mis-tap never stops the recording");
    let (code, _) = rig.press(json!({"action": "stop", "held": true}));
    assert_eq!(code, 200);
    assert!(!rig.booth.view.recording);
    rig.run_until("the take lands", Duration::from_secs(10), |r| !r.landed().is_empty());
    assert!(!rig.piano.view.recording, "the piano PC stopped with the booth");
    assert_lands(&rig, &played);
    assert!(rig.hub.sketches().is_empty(), "the exact take replaced the sketch");
    assert_eq!(rig.booth.e.undo_label(), Some("Record"), "one undo step");
    // The piano PC kept its own copy.
    let local: usize = rig.piano.e.session().tracks.iter().flat_map(|t| t.clips()).filter_map(|c| c.midi_playing()).map(|m| m.notes.len()).sum();
    assert_eq!(local, played.len());
    rig.run_until("the page shows the take received", Duration::from_secs(5), |r| {
        r.hub.remotes().iter().any(|x| x.name == "Piano PC" && x.state == "take received")
    });
    rig.follower.stop();
    rig.hub.stop();
}

#[test]
fn a_booth_count_off_starts_the_piano_pc_at_the_record_point_not_at_the_press() {
    let mut rig = Rig::new(500.0, None);
    // Both PCs have Count Off on: the booth counts off; the piano PC follows the booth's take,
    // which has started by then, so it must not count off again.
    let countoff = Duration::from_millis(600);
    rig.booth.countoff = Some(countoff);
    rig.piano.countoff = Some(Duration::from_secs(5));
    rig.run_until("the piano PC connects", Duration::from_secs(10), |r| r.follower.connected());
    rig.booth.e.session_mut().edit.selection = Range::point(24_000);
    let pressed = Instant::now();
    let (code, r) = rig.press(json!({"action": "record"}));
    assert_eq!(code, 200, "{r}");
    assert!(rig.booth.view.counting_off, "the booth counts off");
    // During the count-off nothing has started anywhere: the booth's page and the piano PC wait.
    while pressed.elapsed() < countoff - Duration::from_millis(150) {
        rig.frame();
        assert!(!rig.piano.view.recording, "the piano PC must not start at the Record press");
        std::thread::sleep(Duration::from_millis(5));
    }
    rig.run_until("the piano PC records", Duration::from_secs(5), |r| r.piano.view.recording);
    assert!(pressed.elapsed() >= countoff, "the piano PC started after the count-off");
    assert!(!rig.piano.view.counting_off, "the piano PC records at once, without its own count-off");
    // At the booth's position, which rolled from the record point.
    rig.booth.tick();
    rig.piano.tick();
    let gap = (rig.piano.view.position - rig.booth.view.position).abs();
    assert!(gap < 4_800, "the piano PC started {gap} samples away from the booth");
    assert!(rig.piano.view.play_from >= 24_000 && rig.piano.view.play_from < 24_000 + 4_800, "{}", rig.piano.view.play_from);
    let mut played = Vec::new();
    for (i, pitch) in [60u8, 67, 72].into_iter().enumerate() {
        rig.run(Duration::from_millis(250 + 60 * i as u64));
        played.push((pitch, rig.note(pitch)));
    }
    rig.run(Duration::from_millis(300));
    let (code, _) = rig.press(json!({"action": "stop", "held": true}));
    assert_eq!(code, 200);
    rig.run_until("the take lands", Duration::from_secs(10), |r| !r.landed().is_empty());
    // The take clock's zero is the record point (the booth's anchor), not the Record press.
    assert_lands(&rig, &played);
    rig.follower.stop();
    rig.hub.stop();
}

#[test]
fn a_pulled_cable_mid_take_keeps_recording_and_the_take_transfers_on_reconnect() {
    let dir = std::env::temp_dir().join(format!("freedawm-outbox-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut rig = Rig::new(100.0, Some(dir.clone()));
    rig.run_until("connected", Duration::from_secs(10), |r| r.follower.connected());
    rig.press(json!({"action": "record"}));
    rig.run_until("the piano PC records", Duration::from_secs(5), |r| r.piano.view.recording);
    rig.run(Duration::from_millis(500));
    let mut played = vec![(60, rig.note(60))];
    rig.cable.pull();
    rig.run_until("the link drops", Duration::from_secs(20), |r| !r.follower.connected());
    // Away: the piano PC keeps recording; the page says the piano PC is not reachable.
    rig.run(Duration::from_millis(300));
    played.push((62, rig.note(62)));
    assert!(rig.piano.view.recording);
    let warn = rig.hub.status().warning.unwrap_or_default();
    assert!(warn.contains("Piano PC not reachable"), "{warn}");
    // The booth stops while the cable is out.
    let (code, _) = rig.press(json!({"action": "stop", "held": true}));
    assert_eq!(code, 200);
    rig.run(Duration::from_millis(300));
    assert!(rig.piano.view.recording, "it hasn't heard yet");
    rig.cable.plug();
    // Back: it hears the booth stopped, ends its take there, and sends it.
    rig.run_until("the take lands after the reconnect", Duration::from_secs(20), |r| !r.landed().is_empty());
    assert_lands(&rig, &played);
    // Sent once and gone from the outbox (on disk too).
    rig.run_until("the outbox empties", Duration::from_secs(5), |r| r.follower.queued() == 0);
    assert_eq!(std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0), 0);
    rig.follower.stop();
    rig.hub.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_take_waiting_to_be_sent_is_kept_in_the_piano_pcs_project_folder() {
    let root = std::env::temp_dir().join(format!("freedawm-project-takes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (outbox, project) = (root.join("Remote Outbox"), root.join("Sunday"));
    let mut rig = Rig::new(100.0, Some(outbox.clone()));
    rig.piano.project = Some(project.clone());
    rig.run_until("connected", Duration::from_secs(10), |r| r.follower.connected());
    rig.press(json!({"action": "record"}));
    rig.run_until("the piano PC records", Duration::from_secs(5), |r| r.piano.view.recording);
    assert_eq!(rig.pglue.badge(&rig.follower).text(), "Recording with Booth");
    rig.run(Duration::from_millis(300));
    let played = vec![(64, rig.note(64))];
    // The booth goes away before the take is sent: it waits, kept in the project, the outbox
    // pointing at it.
    rig.cable.pull();
    rig.run_until("the link drops", Duration::from_secs(20), |r| !r.follower.connected());
    rig.piano.run("transport.stop", json!({})).unwrap();
    rig.run_until("the take is queued", Duration::from_secs(5), |r| r.follower.queued() == 1);
    assert_eq!(rig.pglue.badge(&rig.follower).text(), "Waiting to send take…");
    let kept: Vec<_> = std::fs::read_dir(project.join("Remote Takes")).unwrap().flatten().map(|e| e.path()).collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0].extension().unwrap(), "json");
    let pointers: Vec<_> = std::fs::read_dir(&outbox).unwrap().flatten().map(|e| e.path()).collect();
    assert_eq!(pointers.len(), 1, "{pointers:?}");
    assert_eq!(pointers[0].extension().unwrap(), "ptr");
    let pointer: Value = serde_json::from_slice(&std::fs::read(&pointers[0]).unwrap()).unwrap();
    assert_eq!(pointer["take"].as_str().map(std::path::PathBuf::from), Some(kept[0].clone()));
    // A restarted app finds it through the pointer.
    let again = Follower::start(rig.follower.target(), Some(outbox.clone()), None);
    assert_eq!(again.queued(), 1);
    again.stop();
    let (code, _) = rig.press(json!({"action": "stop", "held": true}));
    assert_eq!(code, 200);
    rig.cable.plug();
    rig.run_until("the take lands after the reconnect", Duration::from_secs(20), |r| !r.landed().is_empty());
    assert_lands(&rig, &played);
    rig.run_until("the outbox empties", Duration::from_secs(5), |r| r.follower.queued() == 0);
    rig.run(Duration::from_millis(50));
    assert_eq!(rig.pglue.badge(&rig.follower).text(), "Take sent to Booth");
    assert!(rig.piano.said.iter().any(|s| s == "Take sent to Booth"), "{:?}", rig.piano.said);
    // The pointer is gone; the take stays in the project.
    assert_eq!(std::fs::read_dir(&outbox).map(|d| d.count()).unwrap_or(0), 0);
    assert!(kept[0].is_file());
    rig.follower.stop();
    rig.hub.stop();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_unreachable_piano_pc_is_reported_and_joins_late_with_a_partial_take() {
    let mut rig = Rig::new(100.0, None);
    rig.run_until("connected", Duration::from_secs(10), |r| r.follower.connected());
    rig.cable.pull();
    rig.run_until("away", Duration::from_secs(20), |r| !r.follower.connected() && !r.hub.missing_followers().is_empty());
    rig.press(json!({"action": "record"}));
    assert!(rig.booth.view.recording, "the booth records anyway");
    rig.bglue.publish(&rig.booth, &rig.hub, true);
    assert!(rig.hub.status().warning.unwrap_or_default().contains("Piano PC not reachable"));
    rig.run(Duration::from_millis(1500));
    assert!(!rig.piano.view.recording);
    rig.cable.plug();
    rig.run_until("the piano PC joins", Duration::from_secs(20), |r| r.piano.view.recording);
    assert!(rig.piano.said.iter().any(|s| s.contains("joined late")), "{:?}", rig.piano.said);
    rig.run(Duration::from_millis(300));
    let played = vec![(65, rig.note(65))];
    rig.run(Duration::from_millis(300));
    rig.bglue.publish(&rig.booth, &rig.hub, true);
    assert!(rig.hub.status().warning.is_none(), "back: no warning");
    rig.press(json!({"action": "stop", "held": true}));
    rig.run_until("the partial take lands", Duration::from_secs(10), |r| !r.landed().is_empty());
    assert_lands(&rig, &played);
    assert!(rig.booth.said.iter().any(|s| s.contains("partial")), "{:?}", rig.booth.said);
    rig.follower.stop();
    rig.hub.stop();
}

#[test]
fn a_booth_away_past_the_contact_timeout_ends_the_take_on_the_piano_pc() {
    let mut rig = Rig::new(100.0, None);
    rig.timeout = Duration::from_millis(1500);
    rig.run_until("connected", Duration::from_secs(10), |r| r.follower.connected());
    rig.press(json!({"action": "record"}));
    rig.run_until("recording", Duration::from_secs(5), |r| r.piano.view.recording);
    rig.run(Duration::from_millis(400));
    let played = vec![(70, rig.note(70))];
    rig.cable.pull();
    rig.run_until("the piano PC gives up on the booth", Duration::from_secs(30), |r| !r.piano.view.recording);
    assert_eq!(rig.follower.queued(), 1, "kept for the booth");
    rig.press(json!({"action": "stop", "held": true}));
    rig.cable.plug();
    rig.run_until("sent when the booth is back", Duration::from_secs(20), |r| !r.landed().is_empty());
    assert_lands(&rig, &played);
    rig.follower.stop();
    rig.hub.stop();
}

#[test]
fn play_and_stop_follow_too() {
    let mut rig = Rig::new(100.0, None);
    rig.run_until("connected", Duration::from_secs(10), |r| r.follower.connected());
    rig.press(json!({"action": "play"}));
    rig.run_until("the piano PC plays", Duration::from_secs(5), |r| r.piano.view.playing);
    assert!(!rig.piano.view.recording);
    let (code, _) = rig.press(json!({"action": "stop"}));
    assert_eq!(code, 200, "a tap stops playback (only a recording needs the hold)");
    rig.run_until("the piano PC stops", Duration::from_secs(5), |r| !r.piano.view.playing);
    rig.follower.stop();
    rig.hub.stop();
}

#[test]
fn the_page_arms_tracks_and_adds_markers_but_cannot_delete() {
    let mut rig = Rig::new(0.0, None);
    let (code, r) = rig.press(json!({"action": "arm_all", "value": false}));
    assert_eq!(code, 200, "{r}");
    assert!(rig.booth.e.session().tracks.iter().all(|t| !t.mixer.record_arm));
    let id = rig.booth.e.session().tracks[0].id.0;
    rig.press(json!({"action": "arm", "track": id, "value": true}));
    assert!(rig.booth.e.session().tracks[0].mixer.record_arm);
    rig.booth.e.session_mut().edit.selection = Range::point(48_000 * 75);
    let (code, r) = rig.press(json!({"action": "marker"}));
    assert_eq!(code, 200, "{r}");
    let m = rig.booth.e.session().markers[0].clone();
    assert_eq!(m.name, "Marker 1:15", "named with the time");
    rig.press(json!({"action": "rename_marker", "id": m.id.0, "name": "Sermon"}));
    assert_eq!(rig.booth.e.session().markers[0].name, "Sermon");
    for bad in [json!({"action": "delete_marker", "id": m.id.0}), json!({"action": "command", "command": "track.delete"})] {
        let (code, _) = rig.press(bad);
        assert_eq!(code, 409);
    }
    assert_eq!(rig.booth.e.session().markers.len(), 1);
    assert_eq!(rig.booth.e.session().tracks.len(), 2);
    // The status the page gets.
    rig.bglue.publish(&rig.booth, &rig.hub, true);
    let st = rig.hub.status();
    assert_eq!(st.tracks.len(), 2);
    assert_eq!(st.tracks[1].remote.as_deref(), Some("Piano PC"), "the piano track says where it comes from");
    assert_eq!(st.markers[0].name, "Sermon");
    rig.follower.stop();
    rig.hub.stop();
}

// ---------------------------------------------------------------------------------------------
// Remote audio

const GUITAR_PORT: &str = "Remote: Piano PC › In 1";

/// A rig whose remote PC records a guitar on an armed audio track, into `root`; the booth's
/// project is `root/Booth`, with a Guitar track on the remote input.
fn audio_rig(ppm: f64, root: &std::path::Path, outbox: Option<std::path::PathBuf>) -> Rig {
    let _ = std::fs::remove_dir_all(root);
    let mut rig = Rig::new(ppm, outbox);
    rig.booth.project = Some(root.join("Booth"));
    let g = rig.booth.e.execute("track.new", &json!({"kind": "audio", "name": "Guitar"})).unwrap()["tracks"][0].as_u64().unwrap();
    rig.booth.e.execute("track.input", &json!({"tracks": [g], "input": GUITAR_PORT})).unwrap();
    let id = rig.piano.e.execute("track.new", &json!({"kind": "audio", "name": "Guitar"})).unwrap()["tracks"][0].as_u64().unwrap();
    rig.piano.e.execute("mix.record_arm", &json!({"tracks": [id], "value": true})).unwrap();
    rig.piano.takes_dir = Some(root.join("Guitar PC").join("Audio Files"));
    rig.piano.project = Some(root.join("Guitar PC"));
    rig
}

impl Rig {
    /// The guitarist plucks now; returns where the booth's timeline is at this instant.
    fn pluck(&mut self) -> Samples {
        self.piano.pluck();
        self.booth.tick();
        self.booth.view.position
    }

    /// Where the plucks landed on the booth's Guitar track (timeline samples), by the peaks of
    /// its clips' audio.
    fn landed_plucks(&self) -> Vec<Samples> {
        let s = self.booth.e.session();
        let t = s.tracks.iter().find(|t| t.name == "Guitar").unwrap();
        let mut v = Vec::new();
        for c in t.clips() {
            let Some(buf) = c.source().and_then(|src| s.pool.get(src)) else { continue };
            let ch = &buf.buffer().channels[0];
            let off = c.source_offset();
            let mut i = off.max(0) as usize;
            let end = (off + c.length).max(0) as usize;
            while i < end.min(ch.len()) {
                if ch[i].abs() > 0.45 {
                    // The peak of this pluck.
                    let mut best = i;
                    let mut j = i;
                    while j < ch.len() && j < i + 64 {
                        if ch[j].abs() > ch[best].abs() {
                            best = j;
                        }
                        j += 1;
                    }
                    v.push(c.start + best as Samples - off);
                    i = best + 200;
                } else {
                    i += 1;
                }
            }
        }
        v.sort();
        v
    }
}

fn assert_plucks(rig: &Rig, played: &[Samples]) -> Vec<Samples> {
    let landed = rig.landed_plucks();
    assert_eq!(landed.len(), played.len(), "landed {landed:?}, played {played:?}");
    let mut offs = Vec::new();
    for (want, got) in played.iter().zip(&landed) {
        eprintln!("pluck: {} samples off", got - want);
        assert!((got - want).abs() <= TOLERANCE, "pluck landed at {got}, played at {want} ({} samples off)", got - want);
        offs.push(got - want);
    }
    offs
}

#[test]
fn a_remote_audio_take_lands_aligned_on_the_booth_track() {
    let root = std::env::temp_dir().join(format!("freedawm-remote-audio-{}", std::process::id()));
    let mut rig = audio_rig(2000.0, &root, None);
    rig.run_until("connected", Duration::from_secs(10), |r| r.follower.connected());
    rig.booth.e.session_mut().edit.selection = Range::point(24_000);
    rig.press(json!({"action": "record"}));
    rig.run_until("the guitar PC records", Duration::from_secs(5), |r| r.piano.view.recording);
    // It armed nothing of its own (its Guitar track is armed): no Piano track made.
    assert!(!rig.piano.e.session().tracks.iter().any(|t| t.name == "Piano"));
    let mut played = Vec::new();
    for i in 0..5u64 {
        rig.run(Duration::from_millis(250 + 60 * i));
        played.push(rig.pluck());
    }
    rig.run(Duration::from_millis(400));
    // The booth sees its live level, loud at the plucks.
    let sk = rig.hub.audio_sketches();
    assert_eq!(sk.first().map(|s| s.port.as_str()), Some(GUITAR_PORT), "{sk:?}");
    assert!(sk[0].peaks.iter().filter(|p| p.1 > 0.5).count() >= 4, "{:?}", sk[0].peaks);
    assert!(rig.hub.remote_inputs().contains(&GUITAR_PORT.to_string()), "offered in the input menu");
    rig.press(json!({"action": "stop", "held": true}));
    rig.run_until("the take lands", Duration::from_secs(20), |r| !r.landed_plucks().is_empty());
    assert_plucks(&rig, &played);
    assert_eq!(rig.booth.e.undo_label(), Some("Record"));
    assert!(rig.booth.said.iter().any(|s| s.contains("take received (In 1)")), "{:?}", rig.booth.said);
    // The file is the booth project's: in its Audio Files, 24-bit WAV at the session rate.
    let files: Vec<_> = std::fs::read_dir(root.join("Booth").join("Audio Files")).unwrap().flatten().map(|e| e.path()).collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let info = soundcraft_audio_io::probe(&std::fs::read(&files[0]).unwrap(), Some("wav")).unwrap();
    assert_eq!((info.sample_rate, info.sample_format), (48_000, soundcraft_audio_io::SampleFormat::Int24));
    // The transfer file arrived in the booth project's Remote Takes; the guitar PC deleted its own
    // once it landed (its WAV stays in its project).
    assert!(std::fs::read_dir(root.join("Booth").join("Remote Takes")).unwrap().flatten().any(|e| e.path().extension().is_some_and(|x| x == "flac")));
    rig.run_until("the outbox empties", Duration::from_secs(5), |r| r.follower.queued() == 0);
    let kept: Vec<_> = std::fs::read_dir(root.join("Guitar PC").join("Remote Takes")).unwrap().flatten().map(|e| e.path()).collect();
    assert!(kept.iter().all(|p| p.extension().is_some_and(|x| x == "json")), "{kept:?}");
    assert!(std::fs::read_dir(root.join("Guitar PC").join("Audio Files")).unwrap().count() >= 1);
    rig.follower.stop();
    rig.hub.stop();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_pulled_cable_mid_take_still_lands_the_audio_after_the_reconnect() {
    let root = std::env::temp_dir().join(format!("freedawm-remote-audio-cable-{}", std::process::id()));
    let mut rig = audio_rig(100.0, &root, Some(root.join("Outbox")));
    rig.run_until("connected", Duration::from_secs(10), |r| r.follower.connected());
    rig.press(json!({"action": "record"}));
    rig.run_until("recording", Duration::from_secs(5), |r| r.piano.view.recording);
    rig.run(Duration::from_millis(400));
    let mut played = vec![rig.pluck()];
    rig.cable.pull();
    rig.run_until("the link drops", Duration::from_secs(20), |r| !r.follower.connected());
    rig.run(Duration::from_millis(300));
    played.push(rig.pluck());
    rig.press(json!({"action": "stop", "held": true}));
    rig.run(Duration::from_millis(300));
    rig.cable.plug();
    rig.run_until("the take lands after the reconnect", Duration::from_secs(30), |r| !r.landed_plucks().is_empty());
    assert_plucks(&rig, &played);
    rig.follower.stop();
    rig.hub.stop();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn clips_are_counted_live_and_when_the_take_lands() {
    let root = std::env::temp_dir().join(format!("freedawm-remote-audio-clips-{}", std::process::id()));
    let mut rig = audio_rig(0.0, &root, None);
    rig.piano.pluck_level = 1.0;
    rig.run_until("connected", Duration::from_secs(10), |r| r.follower.connected());
    rig.press(json!({"action": "record"}));
    rig.run_until("recording", Duration::from_secs(5), |r| r.piano.view.recording);
    let mut played = Vec::new();
    for _ in 0..3 {
        rig.run(Duration::from_millis(300));
        played.push(rig.pluck());
    }
    rig.run(Duration::from_millis(400));
    // Both screens know at once: the performer's glue counts them, the booth's live sketch has them.
    assert_eq!(rig.pglue.clips.len(), 3, "{:?}", rig.pglue.clips);
    assert_eq!(rig.hub.audio_sketches()[0].clips.len(), 3);
    rig.run_until("the booth's page says so", Duration::from_secs(5), |r| r.hub.remotes().iter().any(|x| x.detail.contains("3 clips") && x.warning));
    rig.press(json!({"action": "stop", "held": true}));
    rig.run_until("the take lands", Duration::from_secs(20), |r| !r.landed_plucks().is_empty());
    assert!(rig.booth.said.iter().any(|s| s.ends_with("3 clips in this take")), "{:?}", rig.booth.said);
    let marks = rig.hub.clip_marks();
    assert_eq!(marks.len(), 3);
    for ((_, at), want) in marks.iter().zip(&played) {
        let off = (at * SR) as Samples - want;
        assert!(off.abs() <= TOLERANCE, "clip mark {off} samples off");
    }
    rig.follower.stop();
    rig.hub.stop();
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------------------------
// The mix stream: a performer hearing the booth over the network

/// The two audio threads, simulated: the booth's tap gets its mix (silence with clicks at
/// `clicks`, booth take-clock frames) as its clock rolls; the performer's stream plays as his
/// clock rolls, and he plucks the instant he hears a click.
/// With a plugin delay compensation of `host_pdc` frames on the booth's mix and `perf_pdc` on the
/// performer's, as the player tags them (`playback::stream`): the booth's block rendered at `at`
/// holds the timeline from `host_pdc` before and is tagged so; the stream the performer hears in
/// his block at `at` rides with his mix of `perf_pdc` before, and his recorder's offset takes
/// `perf_pdc` off his take.
struct StreamSim {
    host_pdc: Samples,
    perf_pdc: Samples,
    tap: Arc<soundcraft_playback::stream::MixTap>,
    input: Arc<soundcraft_playback::stream::StreamIn>,
    clicks: Vec<Samples>,
    host_done: Option<Samples>,
    perf_done: Option<Samples>,
    heard: usize,
}

impl StreamSim {
    fn step(&mut self, rig: &mut Rig) {
        rig.booth.tick();
        rig.piano.tick();
        // The booth renders up to its clock.
        match rig.booth.clock.now() {
            Some(t) => {
                let now = (t * SR) as Samples;
                let mut at = self.host_done.unwrap_or(0);
                while at + 256 <= now {
                    let mut l = vec![0f32; 256];
                    for c in &self.clicks {
                        if (at..at + 256).contains(&(c + self.host_pdc)) {
                            l[(c + self.host_pdc - at) as usize] = 0.9;
                        }
                    }
                    self.tap.push_block(&l, &l, 256, at - self.host_pdc, rig.booth.view.play_from + at - self.host_pdc, true);
                    at += 256;
                }
                self.host_done = Some(at);
            }
            None => {
                if self.host_done.take().is_some() {
                    self.tap.push_block(&[0.0], &[0.0], 1, 0, 0, false);
                }
            }
        }
        // The performer's output plays up to his clock.
        if let Some(t) = rig.piano.clock.now() {
            let now = (t * SR) as Samples;
            let mut at = self.perf_done.unwrap_or(0);
            while at + 256 <= now {
                let (mut l, mut r) = (vec![0f32; 256], vec![0f32; 256]);
                self.input.mix_into(&mut l, &mut r, 256, at - self.perf_pdc, true);
                if let Some(i) = l.iter().position(|x| *x > 0.3) {
                    let best = (i..(i + 40).min(256)).max_by(|a, b| l[*a].total_cmp(&l[*b])).unwrap_or(i);
                    // He plays it the instant he hears it: on his own take, at that frame.
                    if rig.piano.view.recording {
                        rig.piano.plucks.push(rig.piano.view.play_from + at + best as Samples);
                        self.heard += 1;
                    }
                }
                at += 256;
            }
            self.perf_done = Some(at);
        } else {
            self.perf_done = None;
        }
    }
}

#[test]
fn plugin_delay_on_either_side_does_not_move_a_stream_take() {
    let root = std::env::temp_dir().join(format!("freedawm-stream-pdc-{}", std::process::id()));
    let mut rig = audio_rig(200.0, &root, None);
    // A 1024-frame lookahead on the booth, 480 on the performer.
    stream_take(&mut rig, 1024, 480);
    rig.follower.stop();
    rig.hub.stop();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_performer_hearing_the_mix_stream_lands_where_he_heard_it() {
    let root = std::env::temp_dir().join(format!("freedawm-stream-take-{}", std::process::id()));
    let mut rig = audio_rig(300.0, &root, None);
    stream_take(&mut rig, 0, 0);
    rig.follower.stop();
    rig.hub.stop();
    let _ = std::fs::remove_dir_all(&root);
}

/// The guitar PC hears the booth through the stream and plays along; its take lands where he
/// played against what he heard.
fn stream_take(rig: &mut Rig, host_pdc: Samples, perf_pdc: Samples) {
    rig.piano.rec_latency = perf_pdc;
    let mut sim = StreamSim {
        host_pdc,
        perf_pdc,
        tap: Arc::new(soundcraft_playback::stream::MixTap::default()),
        input: Arc::new(soundcraft_playback::stream::StreamIn::default()),
        clicks: (1..6).map(|k| 48_000 * k * 6 / 10 + 777).collect(),
        host_done: None,
        perf_done: None,
        heard: 0,
    };
    rig.hub.set_tap(Some(Arc::clone(&sim.tap)), 48_000);
    rig.follower.set_stream(true, Some(Arc::clone(&sim.input)), 48_000);
    rig.run_until("connected", Duration::from_secs(20), |r| r.follower.connected());
    let end = Instant::now() + Duration::from_secs(5);
    while rig.hub.stream_listeners() == 0 {
        assert!(Instant::now() < end, "the guitar PC asks for the mix");
        rig.frame();
        std::thread::sleep(Duration::from_millis(5));
    }
    rig.booth.e.session_mut().edit.selection = Range::point(48_000);
    rig.press(json!({"action": "record"}));
    let until = Instant::now() + Duration::from_millis(4_000);
    while Instant::now() < until {
        rig.frame();
        sim.step(rig);
        std::thread::sleep(Duration::from_millis(3));
    }
    assert_eq!(sim.heard, 5, "he heard every click through the stream");
    let h = rig.follower.stream_health().unwrap();
    eprintln!("stream: buffer {} ms, {} packets, {} lost, {} late", h.buffer_ms, h.packets, h.lost, h.late);
    // The booth stops; he is still hearing the end, his take stops once he has.
    let (code, _) = rig.press(json!({"action": "stop", "held": true}));
    assert_eq!(code, 200);
    let until = Instant::now() + Duration::from_secs(20);
    while rig.landed_plucks().is_empty() {
        assert!(Instant::now() < until, "the take lands; booth said {:?}, guitar PC said {:?}", rig.booth.said, rig.piano.said);
        rig.frame();
        sim.step(rig);
        std::thread::sleep(Duration::from_millis(3));
    }
    // Where the booth played each click, not where (later) he played it on his own clock.
    let played: Vec<Samples> = sim.clicks.iter().map(|c| 48_000 + c).collect();
    let offs = assert_plucks(rig, &played);
    eprintln!("stream take: plucks {offs:?} samples off");
    assert!(offs.iter().all(|o| o.abs() <= 2), "{offs:?}");
}

// ---------------------------------------------------------------------------------------------
// Over the internet transport (iroh, direct on this machine, no relay)

#[test]
fn the_rig_runs_unchanged_over_the_internet_transport_with_an_invite() {
    use soundcraft_net::internet::{Internet, Invite, Relays, new_key};
    let root = std::env::temp_dir().join(format!("freedawm-internet-take-{}", std::process::id()));
    let mut rig = audio_rig(150.0, &root, None);
    rig.follower.stop();
    // The booth: its endpoint, bridged to its internet listener; an invite for the friend.
    let host = Internet::start(new_key(), Relays::LocalOnly).unwrap();
    host.serve_to(rig.hub.listen_internet().unwrap());
    let invite = Invite { addr: host.addr(Duration::from_secs(1)), secret: rig.hub.new_invite_secret(), name: "Booth".into(), role: String::new() };
    let code = invite.code();
    assert_eq!(rig.hub.open_invites().len(), 1);
    // The friend pastes the code: his endpoint, a local bridge to the booth, paired by the invite.
    let friend = Internet::start(new_key(), Relays::LocalOnly).unwrap();
    let got = Invite::parse(&code).unwrap();
    let bridge = friend.bridge_to(got.addr.clone()).unwrap().to_string();
    // A 4-digit code cannot be read across the internet: refused there.
    let refused = soundcraft_net::follower::pair_request(&bridge, "Friend PC");
    assert!(refused.unwrap_err().contains("invite"));
    let (token, booth) = soundcraft_net::follower::pair_invite(&bridge, &got.secret, "Friend PC").unwrap();
    assert_eq!(booth, "Booth");
    assert!(soundcraft_net::follower::pair_invite(&bridge, &got.secret, "Someone else").is_err(), "an invite pairs once");
    let q = friend.quality();
    assert_eq!(q.len(), 1);
    assert!(q[0].direct, "{q:?}");
    eprintln!("internet link: {}", q[0].text());
    // He follows through it: the LAN protocol, unchanged.
    rig.follower = Follower::start(
        FollowTarget { booth_name: String::new(), addr: bridge, token, my_name: "Piano PC".into() },
        None,
        Some(rig.piano.clock.clone()),
    );
    rig.follower.set_ping_every(Duration::from_millis(150));
    stream_take(&mut rig, 0, 0);
    assert!(!host.quality().is_empty());
    rig.follower.stop();
    rig.hub.stop();
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn invites_expire_after_thirty_minutes() {
    let hub = Hub::new(NetConfig::default(), None);
    let port = hub.listen_internet().unwrap();
    let secret = hub.new_invite_secret();
    hub.age_invites(31.0 * 60.0);
    assert!(hub.open_invites().is_empty());
    let err = soundcraft_net::follower::pair_invite(&format!("127.0.0.1:{port}"), &secret, "Friend PC").unwrap_err();
    assert!(err.contains("expired"), "{err}");
    let fresh = hub.new_invite_secret();
    assert!(soundcraft_net::follower::pair_invite(&format!("127.0.0.1:{port}"), &fresh, "Friend PC").is_ok());
    assert!(hub.config().paired.iter().any(|d| d.name == "Friend PC"));
    hub.stop_internet();
}

/// Needs the internet: two endpoints on n0's public relays find each other by key and relay
/// only (no IP addresses in the invite), as two homes would.
#[test]
#[ignore = "needs the internet"]
fn two_pcs_meet_through_the_public_relays() {
    use soundcraft_net::internet::{Internet, Relays, new_key};
    let hub = Hub::new(NetConfig { pc_name: "Booth".into(), ..NetConfig::default() }, None);
    let host = Internet::start(new_key(), Relays::Public).unwrap();
    host.serve_to(hub.listen_internet().unwrap());
    let full = host.addr(Duration::from_secs(10));
    let relay = full.relay_urls().next().cloned().expect("the booth reached a public relay");
    eprintln!("booth relay: {relay}");
    let only_relay = iroh::EndpointAddr::new(full.id).with_relay_url(relay);
    let friend = Internet::start(new_key(), Relays::Public).unwrap();
    let bridge = friend.bridge_to(only_relay).unwrap().to_string();
    let secret = hub.new_invite_secret();
    let (_, booth) = soundcraft_net::follower::pair_invite(&bridge, &secret, "Friend PC").unwrap();
    assert_eq!(booth, "Booth");
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(500));
        if let Some(q) = friend.quality().first() {
            eprintln!("link: {}", q.text());
        }
    }
    hub.stop_internet();
}
