//! The recorder and a real booth in one process: the booth's hub on loopback (a plain link, as
//! `crates/net/tests/rig.rs` runs it), the recorder pairing with its code as a user would, then
//! following a take (audio and MIDI, landing on the booth's tracks on its ports), hearing the
//! booth's mix, and never listening on a socket of its own.

mod common;

use common::{RATE, offline, ramp, temp_dir};
use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_net::config::NetConfig;
use soundcraft_net::glue::{BoothGlue, HostView, RigHost};
use soundcraft_net::hub::{Hub, TakeClock};
use soundcraft_recorder::booth::Link;
use soundcraft_recorder::rig::InputChoice;
use soundcraft_time::Samples;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const ME: &str = "Test Recorder";

/// The booth's audio clock: frames rolled since Record, at real time.
#[derive(Default)]
struct Clock(Mutex<Option<Instant>>);

impl TakeClock for Clock {
    fn now(&self) -> Option<f64> {
        self.0.lock().unwrap().map(|t| t.elapsed().as_secs_f64())
    }
}

/// The booth: an engine with a simulated transport and a project folder (what the DAW's frame
/// does with transport requests, as in `crates/net/tests/rig.rs`).
struct Booth {
    e: Engine,
    clock: Arc<Clock>,
    view: HostView,
    project: std::path::PathBuf,
}

impl Booth {
    fn tick(&mut self) {
        if let Some(t) = self.clock.now() {
            self.view.position = self.view.play_from + (t * f64::from(RATE)) as Samples;
        }
    }
}

impl RigHost for Booth {
    fn engine(&self) -> &Engine {
        &self.e
    }
    fn engine_mut(&mut self) -> &mut Engine {
        &mut self.e
    }
    fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        self.tick();
        match id {
            "transport.record" | "transport.play" => {
                let at = self.e.session().edit.selection.start;
                self.view = HostView {
                    playing: true,
                    recording: id == "transport.record",
                    position: at,
                    play_from: at,
                    record_start: at,
                    ..HostView::default()
                };
                *self.clock.0.lock().unwrap() = Some(Instant::now());
                Ok(json!({}))
            }
            "transport.stop" => {
                (self.view.playing, self.view.recording) = (false, false);
                *self.clock.0.lock().unwrap() = None;
                Ok(json!({}))
            }
            _ => self.e.execute(id, &params).map_err(|e| e.to_string()),
        }
    }
    fn view(&self) -> HostView {
        self.view
    }
    fn project_folder(&self) -> Option<std::path::PathBuf> {
        Some(self.project.clone())
    }
}

struct World {
    hub: Hub,
    port: u16,
    booth: Booth,
    bglue: BoothGlue,
    rec: common::Offline,
    link: Link,
    ideas: std::path::PathBuf,
    listen: bool,
}

impl World {
    fn new(root: &std::path::Path) -> World {
        let hub = Hub::new(NetConfig { pc_name: "Booth".into(), encrypted_only: false, ..NetConfig::default() }, None);
        let port = hub.listen(0).unwrap();
        let clock = Arc::new(Clock::default());
        hub.set_clock(Some(clock.clone() as Arc<dyn TakeClock>));
        let mut e = Engine::default();
        e.session_mut().name = "Sunday".into();
        let vox = e.execute("track.new", &json!({"kind": "audio", "name": "Vox"})).unwrap()["tracks"][0].as_u64().unwrap();
        e.execute("track.input", &json!({"tracks": [vox], "input": format!("Remote: {ME} › In 1")})).unwrap();
        e.execute("mix.record_arm", &json!({"tracks": [vox], "value": true})).unwrap();
        let keys = e.execute("track.new", &json!({"kind": "instrument", "name": "Keys"})).unwrap()["tracks"][0].as_u64().unwrap();
        e.execute("track.input", &json!({"tracks": [keys], "input": format!("midi:Remote: {ME} › Keys")})).unwrap();
        let booth = Booth { e, clock, view: HostView::default(), project: root.join("Booth") };
        let rec = offline(1, &["Keys"], InputChoice { on: true, channel: 0, stereo: false });
        let mut link = Link::new(Some(root.join("recorder-network.json")), Some(root.join("Recorder Outbox")));
        link.cfg.pc_name = ME.into();
        World { hub, port, booth, bglue: BoothGlue::default(), rec, link, ideas: root.join("Ideas"), listen: false }
    }

    /// One frame on both PCs (the recorder's device runs a block at real time).
    fn frame(&mut self) {
        self.rec.block(&ramp);
        self.rec.rig.step();
        self.booth.tick();
        let control = self.bglue.step(&mut self.booth, &self.hub);
        assert!(control.is_empty());
        self.link.step(&mut self.rec.rig, &self.ideas, self.listen, 1.0);
    }

    fn until(&mut self, what: &str, secs: u64, mut done: impl FnMut(&World) -> bool) {
        let end = Instant::now() + Duration::from_secs(secs);
        while !done(self) {
            assert!(Instant::now() < end, "timed out: {what} (link said {:?}, pairing {:?})", self.link.said, self.link.pair_status);
            self.frame();
        }
    }

    fn frames_for(&mut self, secs: f64) {
        let end = Instant::now() + Duration::from_secs_f64(secs);
        while Instant::now() < end {
            self.frame();
        }
    }

    /// The notes landed on the booth's Keys track.
    fn landed_notes(&self) -> Vec<u8> {
        let s = self.booth.e.session();
        let t = s.tracks.iter().find(|t| t.name == "Keys").unwrap();
        t.clips().iter().filter_map(|c| c.midi_playing()).flat_map(|m| m.notes.iter().map(|n| n.pitch)).collect()
    }

    /// Where the first landed note starts on the booth's timeline.
    fn note_at(&self) -> Samples {
        let s = self.booth.e.session();
        let t = s.tracks.iter().find(|t| t.name == "Keys").unwrap();
        let c = t.clips().iter().find(|c| c.midi_playing().is_some()).unwrap();
        let n = c.midi_playing().unwrap().notes[0].start;
        let tick = s.tempo.samples_to_ticks(c.start, s.sample_rate) + n;
        s.tempo.tick_to_samples(tick, s.sample_rate)
    }

    /// Where the Vox take's audio begins on the timeline (its clip's start less what it trims).
    fn vox_start(&self) -> Samples {
        let s = self.booth.e.session();
        let c = &s.tracks.iter().find(|t| t.name == "Vox").unwrap().clips()[0];
        c.start - c.source_offset()
    }

    fn vox_clips(&self) -> usize {
        let s = self.booth.e.session();
        s.tracks.iter().find(|t| t.name == "Vox").map_or(0, |t| t.clips().len())
    }
}

/// The TCP ports this process listens on and whether it holds any UDP socket (Linux: its
/// sockets' inodes in `/proc/self/fd`, matched in `/proc/net`).
#[cfg(target_os = "linux")]
fn my_sockets() -> (Vec<u16>, usize) {
    let mine: Vec<String> = std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .flatten()
        .filter_map(|e| std::fs::read_link(e.path()).ok())
        .filter_map(|l| l.to_str().and_then(|s| s.strip_prefix("socket:[")).map(|s| s.trim_end_matches(']').to_string()))
        .collect();
    let rows = |file: &str| -> Vec<Vec<String>> {
        std::fs::read_to_string(file)
            .unwrap_or_default()
            .lines()
            .skip(1)
            .map(|l| l.split_whitespace().map(str::to_string).collect::<Vec<_>>())
            .filter(|f| f.get(9).is_some_and(|inode| mine.contains(inode)))
            .collect()
    };
    let mut listening = Vec::new();
    for f in ["/proc/net/tcp", "/proc/net/tcp6"] {
        for r in rows(f) {
            if r[3] == "0A" {
                let port = u16::from_str_radix(r[1].rsplit(':').next().unwrap(), 16).unwrap();
                listening.push(port);
            }
        }
    }
    let udp = rows("/proc/net/udp").len() + rows("/proc/net/udp6").len();
    (listening, udp)
}

#[test]
fn the_recorder_pairs_follows_a_take_that_lands_on_the_booth_and_hears_its_mix() {
    let root = temp_dir("booth");
    let mut w = World::new(&root);
    assert!(w.link.follower.is_none(), "nothing is dialled before pairing");

    // Pairing: the address typed, Pair, the code the booth shows typed here.
    w.link.choose("", &format!("127.0.0.1:{}", w.port));
    w.link.pair();
    w.until("the booth shows a code", 10, |w| w.link.wants_code());
    let code = w.hub.pending_pairings().first().unwrap().code.clone();
    assert!(w.hub.pending_pairings().first().unwrap().name == ME, "the booth sees the recorder by its own name");
    w.link.code_edit = code;
    w.link.confirm();
    w.until("paired and following", 20, |w| w.link.paired() && w.link.follower.as_ref().is_some_and(|f| f.connected()));
    assert!(w.hub.config().paired.iter().any(|d| d.name == ME && d.kind == "pc"));
    // Its own settings file, in the DAW's network format, never the DAW's.
    let saved = NetConfig::load(&root.join("recorder-network.json"));
    assert!(!saved.follow.token.is_empty() && saved.follow.enabled && !saved.enabled);

    // The booth records: the recorder records with it.
    w.booth.run("transport.record", json!({})).unwrap();
    w.until("recording with the booth", 10, |w| w.rec.rig.recording());
    w.frames_for(0.4);
    w.booth.tick();
    let played = w.booth.view.position;
    w.rec.keys.send("Keys", &[0x90, 60, 100]);
    w.frames_for(0.3);
    w.rec.keys.send("Keys", &[0x80, 60, 0]);
    w.frames_for(0.4);
    w.booth.run("transport.stop", json!({})).unwrap();
    w.until("the take landed on the booth", 30, |w| w.vox_clips() > 0 && !w.landed_notes().is_empty());
    assert_eq!(w.landed_notes(), vec![60], "the note lands on the booth's track on the recorder's port");
    // Where it was played on the booth's timeline (within the jitter of this test's device,
    // which runs on sleeps), and the audio from where the booth started recording.
    let off_ms = (w.note_at() - played) as f64 * 1000.0 / f64::from(RATE);
    assert!(off_ms.abs() < 40.0, "the note lands where it was played: {off_ms:.1} ms off");
    let start_ms = w.vox_start() as f64 * 1000.0 / f64::from(RATE);
    assert!(start_ms.abs() < 100.0, "the audio take starts where the booth's did: {start_ms:.1} ms");
    assert!(!w.rec.rig.recording());
    // The local copies, in the ideas folder, named after the booth's session.
    let local = soundcraft_recorder::files::recent(&w.ideas, 10);
    assert_eq!(local.len(), 1, "{local:?}");
    assert!(local[0].stem.starts_with("Sunday "), "{}", local[0].stem);
    assert!(local[0].wav.is_some() && local[0].mid.is_some());
    w.until("the outbox emptied", 10, |w| w.link.follower.as_ref().is_some_and(|f| f.queued() == 0));

    // Listening: the booth plays, its mix streams to the recorder and plays in its output.
    w.listen = true;
    let tap = Arc::new(soundcraft_playback::stream::MixTap::default());
    w.hub.set_tap(Some(Arc::clone(&tap)), RATE);
    w.until("the recorder asks for the mix", 10, |w| w.hub.stream_listeners() > 0);
    let tone: Vec<f32> = (0..256).map(|i| 0.3 * (std::f32::consts::TAU * 440.0 * i as f32 / RATE as f32).sin()).collect();
    let (mut rolled, mut heard) = (0i64, 0.0f32);
    let end = Instant::now() + Duration::from_secs(15);
    let input = w.rec.rig.player.stream_in();
    while input.played() < u64::from(RATE) / 4 {
        assert!(Instant::now() < end, "the mix played here ({} frames)", input.played());
        tap.push_block(&tone, &tone, 256, rolled, rolled, true);
        rolled += 256;
        let out = w.rec.block(&ramp);
        heard = heard.max(out.iter().fold(0.0f32, |m, x| m.max(x.abs())));
        w.booth.tick();
        w.bglue.step(&mut w.booth, &w.hub);
        w.link.step(&mut w.rec.rig, &w.ideas, w.listen, 1.0);
    }
    assert!(heard > 0.05, "the booth's mix is heard in the output ({heard})");
    let view = w.link.view(&w.rec.rig).unwrap();
    assert!(view.stream.as_ref().is_some_and(|h| h.packets > 0 && h.buffer_ms > 0.0), "{:?}", view.stream);

    // The recorder listens on nothing: the only listening socket here is the booth's hub, and
    // nothing holds a UDP socket (no mDNS was asked for, the plain link is TCP).
    #[cfg(target_os = "linux")]
    {
        let (listening, udp) = my_sockets();
        assert!(listening.iter().all(|p| *p == w.port), "listening ports: {listening:?} (the booth's is {})", w.port);
        assert_eq!(udp, 0, "no UDP socket");
    }
    w.link.shutdown();
    w.hub.stop();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_booth_away_is_said_and_nothing_is_dialled_before_pairing() {
    let root = temp_dir("away");
    let mut link = Link::new(Some(root.join("recorder-network.json")), None);
    assert!(link.follower.is_none() && !link.paired());
    // Pairing with nobody there says so (no crash, no hang).
    link.choose("", "127.0.0.1:1");
    link.pair();
    let end = Instant::now() + Duration::from_secs(10);
    let mut o = offline(0, &[], InputChoice { on: false, channel: 0, stereo: false });
    while link.pair_status == "Asking the booth…" {
        assert!(Instant::now() < end);
        link.step(&mut o.rig, &root, false, 1.0);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(link.pair_status.contains("cannot reach the booth"), "{}", link.pair_status);
    let _ = std::fs::remove_dir_all(root);
}
