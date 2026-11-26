//! The recorder's one window: one big button. It opens ready to record (screen 1); while
//! recording it turns red with a big clock and Stop (screen 2); on Stop it says the idea is saved
//! and lets it be named, played, opened in FreeDAWm, shown or thrown away (screen 3); "Record
//! with a studio" follows a FreeDAWm PC elsewhere (screen 4). The devices are chosen for the
//! user (the default input and every MIDI keyboard), described in plain words; Change and the
//! gear open a small settings screen.
//!
//! The window paints before any device opens: the devices open on the second frame (they cannot
//! move between threads) and the lists of other devices are read on a thread when the settings
//! are opened, so nothing slow comes before the first paint. Record pressed before then starts
//! as soon as the devices are ready.

pub mod look;
pub mod paint;
mod screens;

use crate::booth::Link;
use crate::files::{self, TakeFiles};
use crate::layers::{self, Sidecar};
use crate::rig::{Cause, InputChoice, Rig, Saved};
use crate::settings::{Paths, Prefs, file_safe, now_stamp, unique_stem};
use crate::words;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

pub use crate::words::clock;

/// The tooltip on the take's MIDI file.
pub const MIDI_TIP: &str = "The piano notes are saved as a MIDI file at 120 bpm, each note at the real time it was played \
(not moved to a grid), so it lines up with the recording in any program.";

/// How many bars a take's shape is drawn with (the design's).
pub const SHAPE_BARS: usize = 22;
/// A name typed in the name box is applied this long after the last key (seconds).
pub const RENAME_AFTER: f64 = 1.0;

/// The audio devices the settings list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceLists {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

/// Which screen shows (recording shows over any of them while a take records).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Screen {
    /// 1: ready to record, the ideas.
    Main,
    /// 3: a take just saved (or one opened from the list).
    Idea,
    /// The small chooser: devices, folder, start-up, the studio.
    Settings,
    /// 4: record with a studio.
    Studio,
}

/// The take screen 3 shows.
#[derive(Debug, Clone)]
pub struct Idea {
    pub take: TakeFiles,
    /// Its name when the screen opened (typing the default name back restores it).
    pub orig_stem: String,
    /// The name the box starts with (`Idea · Monday 2:03 pm`).
    pub default_name: String,
    /// The name box.
    pub name: String,
    /// Just recorded (else opened from the list).
    pub just_saved: bool,
    /// Piano notes in it, when known.
    pub notes: Option<usize>,
    /// What went wrong while it recorded (it is saved up to there).
    pub problem: Option<(Cause, String)>,
    /// Its shape (0..1 peaks), [`SHAPE_BARS`] of them.
    pub shape: Vec<f32>,
    /// When the name box last changed (egui time), until it is applied.
    edited_at: Option<f64>,
    pub name_error: Option<String>,
    /// Throw away asks first.
    pub confirm_throw: bool,
    /// The last thing a button here said ("Opening FreeDAWm…"), and whether it is a problem.
    pub note: Option<(String, bool)>,
    /// How its layers play together (its sidecar's, or the defaults).
    pub side: Sidecar,
    /// The layers a layer take just made (the header says so).
    pub made: Vec<u32>,
    /// Throwing one layer away asks first: which.
    pub confirm_layer: Option<u32>,
}

/// A take playing.
#[derive(Debug, Clone)]
struct Playing {
    stem: String,
    since: Instant,
    seconds: f64,
    /// Round and round (it plays until stopped).
    looped: bool,
}

/// Each number of the 3-2-1 before a layer shows this long (seconds).
pub const COUNT_STEP: f64 = 0.8;

pub struct RecorderApp {
    /// The devices (`None` until they open, on the second frame).
    pub rig: Option<Rig>,
    pub link: Link,
    pub prefs: Prefs,
    paths: Option<Paths>,
    saved_prefs: Prefs,
    pub takes: Vec<TakeFiles>,
    scanned: Option<Instant>,
    pub screen: Screen,
    pub idea: Option<Idea>,
    /// What the main screen says (a problem when true).
    pub message: Option<(String, bool)>,
    playing: Option<Playing>,
    /// The device lists for the settings (read on a thread when they open).
    pub devices: Option<DeviceLists>,
    devices_job: Option<Arc<Mutex<Option<DeviceLists>>>>,
    /// Real devices (not a test's): they open on the second frame and may be reopened.
    live: bool,
    frames: u64,
    /// Record was pressed before the devices were ready (or the setting asks to record at once).
    pub record_when_ready: bool,
    born: Instant,
    /// How long after the app was made its devices were ready to record.
    pub ready_in: Option<Duration>,
    /// Piano notes counted in the take recording, and the next MIDI event to count from.
    notes: (usize, usize),
    /// The studio screen's box (a PIN, a code, an invite or an address).
    pub studio_box: String,
    /// When the studio screen started looking for studios.
    finding_since: Option<Instant>,
    /// FreeDAWm beside this program (`None`: not there).
    pub daw: Option<PathBuf>,
    /// egui's clock this frame.
    now: f64,
    /// The window's place and size, kept at exit.
    pub window: Option<([f32; 2], [f32; 2])>,
    /// Add a layer was pressed: the 3-2-1 counts from then (the layer starts at its end).
    pub counting: Option<Instant>,
    /// The loop a layer records over (frames), while it does.
    layer_loop: i64,
    /// Show the headphones hint while this layer records.
    pub layer_hint: bool,
    /// **Line up** is measuring.
    pub measuring: bool,
    /// What **Line up** last said, and whether it is a problem.
    pub line_up_note: Option<(String, bool)>,
    /// The last taps of **Tap** (egui time, seconds).
    taps: Vec<f64>,
    /// The tempo box while it is being typed in.
    click_typing: Option<String>,
}

/// FreeDAWm's program beside `exe` (the Recorder's own), when it is there.
pub fn daw_beside(exe: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) { "freedawm.exe" } else { "freedawm" };
    exe.parent().map(|d| d.join(name)).filter(|p| p.is_file())
}

/// What FreeDAWm is started with to open a take: its WAV and its MIDI file (the DAW imports the
/// audio and MIDI files named on its command line into a new, untitled session, each at the
/// start: the WAV on an audio track, the MIDI file on an instrument track).
///
/// Every layer goes, in order (each WAV, then its MIDI file): each lands on a track of its own,
/// named after its file (`Chorus idea (layer 2)`), all at the start, so they line up.
pub fn daw_args(take: &TakeFiles) -> Vec<OsString> {
    take.all_layers().into_iter().flat_map(|l| l.wav.into_iter().chain(l.mid)).map(|p| p.into_os_string()).collect()
}

/// A layer as the window names it.
pub fn layer_name(n: u32) -> String {
    if n <= 1 { "First take".into() } else { format!("Layer {n}") }
}

/// Why Line up did not work, in plain words.
fn line_up_problem(e: &str) -> String {
    let lower = e.to_lowercase();
    if lower.contains("no click came back") {
        "No click came back. Hold the microphone near the speaker and turn it up a little (or connect the output to the input with a cable), then press Line up again.".into()
    } else if lower.contains("no audio input") || lower.contains("not on the device") {
        "Line up needs a microphone or input to listen with.".into()
    } else if lower.contains("same rate") {
        "Line up can't run with these devices (they run at different rates).".into()
    } else {
        format!("Line up didn't work: {e}")
    }
}

fn choice_of(p: &Prefs) -> InputChoice {
    InputChoice { on: p.audio_on, channel: p.channel, stereo: p.stereo }
}

impl RecorderApp {
    /// The app on the real devices, with the settings in `paths`. Opens no device: the first
    /// frame paints, the devices open on the second.
    pub fn open(paths: Option<Paths>) -> RecorderApp {
        let prefs = paths.as_ref().map(|p| Prefs::load(&p.prefs)).unwrap_or_default();
        let mut app = RecorderApp::new(None, prefs, paths);
        app.live = true;
        app.daw = std::env::current_exe().ok().as_deref().and_then(daw_beside);
        app
    }

    /// The app on `rig` (tests), with settings `prefs` and no settings files.
    pub fn with_rig(rig: Rig, prefs: Prefs, paths: Option<Paths>) -> RecorderApp {
        let mut app = RecorderApp::new(Some(rig), prefs, paths);
        app.ready_in = Some(Duration::ZERO);
        app.record_when_ready = app.prefs.record_on_open;
        app
    }

    /// The app before its devices open (tests of the first frames).
    #[doc(hidden)]
    pub fn without_rig(prefs: Prefs) -> RecorderApp {
        RecorderApp::new(None, prefs, None)
    }

    fn new(rig: Option<Rig>, prefs: Prefs, paths: Option<Paths>) -> RecorderApp {
        let link = Link::new(paths.as_ref().map(|p| p.network.clone()), paths.as_ref().map(|p| p.outbox.clone()));
        RecorderApp {
            rig,
            link,
            saved_prefs: prefs.clone(),
            prefs,
            paths,
            takes: Vec::new(),
            scanned: None,
            screen: Screen::Main,
            idea: None,
            message: None,
            playing: None,
            devices: None,
            devices_job: None,
            live: false,
            frames: 0,
            record_when_ready: false,
            born: Instant::now(),
            ready_in: None,
            notes: (0, 0),
            studio_box: String::new(),
            finding_since: None,
            daw: None,
            now: 0.0,
            window: None,
            counting: None,
            layer_loop: 0,
            layer_hint: false,
            measuring: false,
            line_up_note: None,
            taps: Vec::new(),
            click_typing: None,
        }
    }

    /// The devices are open (the window can record).
    pub fn ready(&self) -> bool {
        self.rig.is_some()
    }

    /// A take is recording.
    pub fn recording(&self) -> bool {
        self.rig.as_ref().is_some_and(Rig::recording)
    }

    /// Hands the app its devices (the second frame, or a test's).
    pub fn set_rig(&mut self, rig: Rig) {
        self.rig = Some(rig);
        self.ready_in = Some(self.born.elapsed());
        if self.prefs.record_on_open {
            self.record_when_ready = true;
        }
    }

    fn open_rig(&mut self) {
        let midi = self.prefs.midi.clone();
        let rig = Rig::open(&self.prefs.audio, choice_of(&self.prefs), midi);
        log::info!("recorder: devices ready {:.0} ms after start", self.born.elapsed().as_secs_f64() * 1000.0);
        self.set_rig(rig);
    }

    /// Reopens the devices as the settings say (a device picked, Try again).
    fn reopen(&mut self) {
        if !self.live || self.recording() {
            return;
        }
        let midi = match &self.rig {
            Some(r) => {
                let _ = r.play_file(None);
                r.midi_choices()
            }
            None => self.prefs.midi.clone(),
        };
        self.playing = None;
        // The old devices close before the new ones open.
        self.rig = None;
        self.rig = Some(Rig::open(&self.prefs.audio, choice_of(&self.prefs), midi));
        if self.screen == Screen::Settings {
            self.read_devices();
        }
    }

    /// Reads the device lists on a thread (the settings show them when they come).
    fn read_devices(&mut self) {
        if !self.live {
            return;
        }
        let live = self.rig.as_ref().map(|r| r.player.live_device());
        let job = Arc::new(Mutex::new(None));
        let out = Arc::clone(&job);
        let system = self.prefs.audio.system_name();
        let spawned = std::thread::Builder::new().name("recorder-devices".into()).spawn(move || {
            let systems = soundcraft_playback::devices::audio_systems_with(live.as_ref());
            let sys = systems.iter().find(|s| s.name == system).or_else(|| systems.iter().find(|s| s.is_default));
            let lists = DeviceLists {
                inputs: sys.map(|s| s.inputs.iter().map(|d| d.name.clone()).collect()).unwrap_or_default(),
                outputs: sys.map(|s| s.outputs.iter().map(|d| d.name.clone()).collect()).unwrap_or_default(),
            };
            *out.lock().unwrap_or_else(PoisonError::into_inner) = Some(lists);
        });
        if spawned.is_ok() {
            self.devices_job = Some(job);
        }
    }

    /// Once a frame, before drawing (`now`: egui's clock).
    pub fn step(&mut self, now: f64) {
        self.now = now;
        self.frames = self.frames.saturating_add(1);
        if self.live && self.rig.is_none() && self.frames >= 2 {
            self.open_rig();
        }
        let saved = self.rig.as_mut().and_then(Rig::step);
        if let Some(s) = saved {
            self.on_saved(s);
        }
        self.tune_rig();
        if self.counting.is_some_and(|t| t.elapsed().as_secs_f64() >= 3.0 * COUNT_STEP) {
            self.counting = None;
            self.begin_layer();
        }
        self.poll_line_up();
        if self.record_when_ready && self.rig.is_some() {
            self.record_when_ready = false;
            if !self.recording() {
                self.start();
            }
        }
        let folder = self.prefs.ideas();
        if let Some(rig) = self.rig.as_mut() {
            let was = rig.recording();
            self.link.step(rig, &folder, self.prefs.listen, self.prefs.listen_gain);
            if was != rig.recording() {
                self.notes = (0, 0);
                self.scanned = None;
            }
            if rig.recording() {
                let (n, next) = rig.notes_since(self.notes.1);
                self.notes = (self.notes.0 + n, next);
            }
            self.prefs.midi = rig.midi_choices();
        }
        if self.prefs != self.saved_prefs {
            self.save_prefs();
        }
        if self.scanned.is_none_or(|t| t.elapsed() >= Duration::from_secs(2)) {
            self.rescan();
        }
        if let Some(job) = &self.devices_job {
            let got = job.lock().unwrap_or_else(PoisonError::into_inner).take();
            if got.is_some() {
                self.devices = got;
                self.devices_job = None;
            }
        }
        if self.playing.as_ref().is_some_and(|p| !p.looped && p.since.elapsed().as_secs_f64() > p.seconds + 0.3) {
            self.playing = None;
        }
        if self.idea.as_ref().and_then(|i| i.edited_at).is_some_and(|t| now - t >= RENAME_AFTER) {
            self.apply_name();
        }
    }

    fn save_prefs(&mut self) {
        if let Some(p) = &self.paths
            && let Err(e) = self.prefs.save(&p.prefs)
        {
            self.message = Some((format!("Your settings could not be saved: {e}"), true));
        }
        self.saved_prefs = self.prefs.clone();
    }

    pub fn rescan(&mut self) {
        let dir = self.prefs.ideas();
        // A layer a crash left half done is finished first (never the one recording now).
        if self.takes.iter().any(|t| t.sidecar.is_some()) || self.scanned.is_none() {
            let busy = self.rig.as_ref().and_then(Rig::layering).map(|(i, _)| i.to_string());
            for (idea, r) in layers::recover(&dir, busy.as_deref()) {
                log::info!("recorder: finished a layer of “{idea}” left by a crash: {r:?}");
            }
        }
        self.takes = files::recent(&dir, 200);
        self.scanned = Some(Instant::now());
    }

    /// The rig's layer offset from the settings: the nudge, and the round trip **Line up**
    /// measured on these very devices.
    fn tune_rig(&mut self) {
        let Some(rig) = self.rig.as_mut() else { return };
        rig.nudge = (f64::from(self.prefs.layer_nudge_ms) * f64::from(rig.rate()) / 1000.0).round() as i64;
        let key = rig.latency_key();
        rig.measured = self.prefs.layer_measured.as_ref().filter(|m| m.key == key).map(|m| (m.frames, m.feed));
        // (No change while a take records.)
        rig.set_click(self.prefs.click());
    }

    /// **Line up**: plays a click out of the output and listens for it on the input (a cable, or
    /// the microphone near the speaker); the round trip found places layers from then on.
    pub fn line_up(&mut self) {
        let Some(rig) = self.rig.as_ref() else { return };
        if self.recording() || self.measuring {
            return;
        }
        if let Some(r) = &self.rig {
            let _ = r.play_file(None);
        }
        self.playing = None;
        match rig.player.measure_latency(0, self.prefs.channel) {
            Ok(()) => {
                self.measuring = true;
                self.line_up_note = Some(("Listening for the click…".into(), false));
            }
            Err(e) => self.line_up_note = Some((line_up_problem(&e), true)),
        }
    }

    fn poll_line_up(&mut self) {
        if !self.measuring {
            return;
        }
        let Some(rig) = self.rig.as_ref() else {
            self.measuring = false;
            return;
        };
        let Some(r) = rig.player.measure_result() else { return };
        self.measuring = false;
        match r {
            Ok(frames) => {
                let ms = frames as f64 * 1000.0 / f64::from(rig.rate().max(1));
                self.prefs.layer_measured = Some(crate::settings::Measured { key: rig.latency_key(), frames, feed: rig.player.measure_feed() });
                self.line_up_note = Some((format!("Lined up. Sound takes {ms:.0} ms to go out and come back in on this computer."), false));
            }
            Err(e) => self.line_up_note = Some((line_up_problem(&e), true)),
        }
    }

    /// The ideas listed (not the one recording now).
    pub fn ideas(&self) -> Vec<&TakeFiles> {
        let current = self.rig.as_ref().and_then(Rig::current_stem);
        self.takes.iter().filter(|t| Some(t.stem.as_str()) != current).collect()
    }

    /// The window closes: a take recording is stopped and saved, a name typed applied.
    pub fn shutdown(&mut self) {
        self.apply_name();
        if let Some(r) = self.rig.as_mut().filter(|r| r.recording()) {
            let _ = r.stop();
        }
        self.link.shutdown();
        if let Some((pos, size)) = self.window {
            self.prefs.window_pos = Some(pos);
            self.prefs.window_size = Some(size);
        }
        if self.prefs != self.saved_prefs {
            self.save_prefs();
        }
    }

    /// Record (or Stop), as the big button and Space do.
    pub fn toggle_record(&mut self) {
        if self.counting.take().is_some() {
            // Stop during the 3-2-1: no layer.
            return;
        }
        if self.recording() {
            self.stop();
        } else {
            self.start();
        }
    }

    fn start(&mut self) {
        self.apply_name();
        let Some(rig) = self.rig.as_mut() else {
            // The devices are opening: it starts as soon as they are ready.
            self.record_when_ready = true;
            return;
        };
        self.playing = None;
        let _ = rig.play_file(None);
        let dir = self.prefs.ideas();
        let stem = unique_stem(&dir, "Idea", now_stamp());
        match rig.start(&dir, &stem, false, 0) {
            Ok(()) => {
                self.message = None;
                self.notes = (0, 0);
                if let Some(i) = &mut self.idea {
                    i.confirm_throw = false;
                }
            }
            Err(e) => self.message = Some((start_problem(&e, &dir), true)),
        }
    }

    fn stop(&mut self) {
        let Some(rig) = self.rig.as_mut() else { return };
        let for_studio = rig.recording_for_booth();
        let saved = rig.stop();
        if for_studio {
            // The studio's take: it is sent to the studio (and kept here); the screen stays.
            if let Some(p) = saved.problem {
                self.link.said = Some(p);
            }
            self.scanned = None;
            return;
        }
        self.on_saved(saved);
    }

    /// A take stopped: screen 3 shows it.
    fn on_saved(&mut self, s: Saved) {
        if let Some(raw) = s.layer.clone() {
            self.on_layer_saved(&raw, s);
            return;
        }
        self.rescan();
        let Some(path) = s.wav.as_ref().or(s.mid.as_ref()) else {
            self.message = Some((s.problem.unwrap_or_else(|| "Nothing was recorded: no sound and no piano notes came in.".into()), true));
            self.screen = Screen::Main;
            return;
        };
        let stem = path.file_stem().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        // Recorded to the click: the idea knows its tempo, and is a whole number of bars long
        // (so it loops in time) unless that is turned off.
        let mut looped_as = None;
        if let Some((c, rate)) = self.rig.as_ref().and_then(|r| r.click().map(|c| (c, r.rate()))) {
            if self.prefs.click_round_bars
                && let Some(w) = &s.wav
            {
                looped_as = layers::round_to_bars(w, c.bar_frames(rate)).ok();
            }
            let side = Sidecar { tempo: Some(c), ..Sidecar::default() };
            let at = path.with_file_name(format!("{stem}.{}", layers::SIDECAR_EXT));
            if let Err(e) = side.save(&at) {
                log::warn!("recorder: {}: {e}", at.display());
            }
            self.rescan();
        }
        let take = self
            .takes
            .iter()
            .find(|t| t.stem == stem)
            .cloned()
            .unwrap_or_else(|| TakeFiles::single(&stem, s.wav.clone(), s.mid.clone(), Some(s.seconds)));
        let mut idea = self.idea_for(take);
        idea.just_saved = true;
        idea.notes = s.mid.as_ref().map(|_| s.notes);
        idea.problem = s.problem.map(|p| (s.cause.unwrap_or(Cause::Other), p));
        if let Some((Cause::Disk, _)) = idea.problem {
            let full = soundcraft_net::sys::disk_free(&self.prefs.ideas()).is_some_and(|b| b < 64 << 20);
            idea.problem = Some((Cause::Disk, if full { "full".into() } else { "slow".into() }));
        }
        if let Some(bars) = looped_as {
            idea.note = Some((if bars == 1 { "Looped as 1 bar".to_string() } else { format!("Looped as {bars} bars") }, false));
        }
        self.idea = Some(idea);
        self.screen = Screen::Idea;
    }

    /// **Tap**: the click's tempo from the last few taps (`now`: egui's clock, seconds). A tap
    /// more than 2 seconds after the last starts again.
    pub fn tap(&mut self, now: f64) {
        if self.taps.last().is_some_and(|t| now - t > 2.0 || now < *t) {
            self.taps.clear();
        }
        self.taps.push(now);
        let n = self.taps.len();
        if n > 5 {
            self.taps.drain(..n - 5);
        }
        if let (Some(first), Some(last)) = (self.taps.first(), self.taps.last())
            && self.taps.len() >= 2
        {
            let each = (last - first) / (self.taps.len() - 1) as f64;
            if each > 0.0 {
                self.prefs.click_bpm = (60.0 / each).round().clamp(crate::rig::Click::MIN_BPM, crate::rig::Click::MAX_BPM);
            }
        }
    }

    /// A layer take stopped: it is cut into its layers ([`layers::finish`]) and screen 3 shows
    /// the idea with them.
    fn on_layer_saved(&mut self, raw: &layers::LayerRaw, s: Saved) {
        self.layer_hint = false;
        let res = layers::finish(raw);
        let side_path = raw.folder.join(format!("{}.{}", raw.idea, layers::SIDECAR_EXT));
        let mut side = Sidecar::load(&side_path);
        side.pending = None;
        if !res.made.is_empty() {
            side.looped = true;
        }
        if let Err(e) = side.save(&side_path) {
            log::warn!("recorder: {}: {e}", side_path.display());
        }
        self.rescan();
        let take = self.takes.iter().find(|t| t.stem == raw.idea).cloned();
        let Some(take) = take else {
            self.message = Some((res.problem.unwrap_or_else(|| "Nothing was played, so no layer was added.".into()), true));
            self.screen = Screen::Main;
            return;
        };
        let mut idea = self.idea_for(take);
        // The name typed before stays (the idea was renamed already).
        idea.just_saved = !res.made.is_empty();
        idea.made = res.made.clone();
        let problem = s.problem.map(|p| (s.cause.unwrap_or(Cause::Other), p));
        idea.problem = problem.or_else(|| res.problem.clone().map(|p| (Cause::Other, p)));
        if res.made.is_empty() && idea.problem.is_none() {
            idea.note = Some(("Nothing was played, so no layer was added.".into(), false));
        } else if res.empty > 0 {
            let n = res.empty;
            idea.note = Some((
                format!("{n} time{} round with nothing played {} not kept.", if n == 1 { "" } else { "s" }, if n == 1 { "was" } else { "were" }),
                false,
            ));
        }
        self.idea = Some(idea);
        self.screen = Screen::Idea;
    }

    /// **Add a layer** on screen 3: after the 3-2-1 (when the setting asks for it) the idea plays
    /// round and round from its start and a new layer records over it.
    pub fn add_layer(&mut self) {
        self.apply_name();
        if self.recording() || self.counting.is_some() || self.idea.is_none() {
            return;
        }
        let Some(rig) = &self.rig else { return };
        if !rig.can_record() {
            return;
        }
        let _ = rig.play_file(None);
        self.playing = None;
        // With the click on, its bar counts in instead of the 3-2-1.
        if self.prefs.count_in && self.prefs.click().is_none() {
            self.counting = Some(Instant::now());
        } else {
            self.begin_layer();
        }
    }

    /// The layer starts now (the count is over).
    fn begin_layer(&mut self) {
        let Some(i) = &mut self.idea else { return };
        let Some(rig) = self.rig.as_mut() else { return };
        i.confirm_throw = false;
        i.confirm_layer = None;
        i.made.clear();
        let fail = |i: &mut Idea, e: String| i.note = Some((e, true));
        let mix = match layers::mix(&i.take, &i.side, rig.rate()) {
            Ok(m) => m,
            Err(e) => return fail(i, e),
        };
        let Some(folder) = i.take.folder() else { return fail(i, "This idea's files are gone.".into()) };
        let first = i.take.next_layer();
        let n = i64::try_from(mix.frames).unwrap_or(i64::MAX);
        // Noted beside the idea while it records: a crash leaves a take that is finished later.
        let side_path = folder.join(format!("{}.{}", i.take.stem, layers::SIDECAR_EXT));
        let mut side = i.side.clone();
        side.pending = Some(layers::Pending { layer: first, loop_frames: n, rate: rig.rate(), offset: rig.layer_offset() });
        if let Err(e) = side.save(&side_path) {
            return fail(i, format!("This idea's folder can't be written to: {e}"));
        }
        // The click at the idea's own tempo when it has one.
        let click = self.prefs.click().map(|c| i.side.tempo.map_or(c, |t| crate::rig::Click { volume_db: c.volume_db, ..t }));
        rig.set_click(click);
        match rig.start_layer(&folder, &i.take.stem, first, mix.channels, n) {
            Ok(()) => {
                i.side = side;
                self.layer_loop = n;
                self.notes = (0, 0);
                // Once: what to do so the idea isn't recorded again from the speakers.
                self.layer_hint = !self.prefs.layer_hint_seen && self.prefs.audio_on && rig.input_channels() > 0;
                self.prefs.layer_hint_seen |= self.layer_hint;
            }
            Err(e) => {
                side.pending = None;
                let _ = side.save(&side_path);
                fail(i, start_problem(&e, &folder));
            }
        }
    }

    /// While a layer records: the idea, the layer number, how far round the loop (0..1) and
    /// which time round (1 for the first).
    pub fn layer_status(&self) -> Option<(String, u32, f32, u32)> {
        let rig = self.rig.as_ref()?;
        let (idea, first) = rig.layering()?;
        let (at, pass) = rig.loop_at(self.layer_loop);
        Some((idea.to_string(), first, at, pass.saturating_add(1)))
    }

    /// The 3-2-1 showing now (3, 2 or 1), while it counts.
    pub fn count_shown(&self) -> Option<u32> {
        let t = self.counting?.elapsed().as_secs_f64();
        Some((3 - (t / COUNT_STEP).floor().clamp(0.0, 2.0) as u32).max(1))
    }

    /// Mutes or unmutes a layer of the idea on screen 3 (remembered in its sidecar).
    pub fn set_muted(&mut self, n: u32, muted: bool) {
        self.edit_mix(n, |m| m.muted = muted, true);
    }

    /// A layer's level (0..2, 1 as recorded); `done`: the slider was let go (the mix playing
    /// picks it up then).
    pub fn set_level(&mut self, n: u32, level: f32, done: bool) {
        self.edit_mix(n, |m| m.level = level.clamp(0.0, 2.0), done);
    }

    /// Loop on screen 3 (remembered in the idea's sidecar).
    pub fn set_looped(&mut self, on: bool) {
        let Some(i) = &mut self.idea else { return };
        i.side.looped = on;
        self.save_side(true);
    }

    fn edit_mix(&mut self, n: u32, f: impl FnOnce(&mut layers::LayerMix), done: bool) {
        let Some(i) = &mut self.idea else { return };
        let mut m = i.side.mix(n);
        f(&mut m);
        i.side.layers.insert(n, m);
        self.save_side(done);
    }

    /// Writes the idea's sidecar; `replay`: a mix playing restarts with the change.
    fn save_side(&mut self, replay: bool) {
        let Some(i) = &mut self.idea else { return };
        if let Some(p) = i.take.sidecar_path() {
            match i.side.save(&p) {
                Ok(()) => i.take.sidecar = Some(p),
                Err(e) => i.note = Some((format!("Couldn't remember that: {e}"), true)),
            }
        }
        self.scanned = None;
        if replay && self.playing.as_ref().is_some_and(|p| p.stem == i.take.stem) {
            let take = i.take.clone();
            self.playing = None;
            self.play(&take);
        }
    }

    /// Throws one layer of the idea on screen 3 away (after it asked); the others stay.
    pub fn throw_away_layer(&mut self, n: u32) {
        self.stop_playing();
        let Some(i) = &mut self.idea else { return };
        i.confirm_layer = None;
        match files::delete_layer(&i.take, n) {
            Ok(t) => {
                i.take = t;
                i.side.layers.remove(&n);
                i.made.retain(|m| *m != n);
                i.shape = i.take.all_layers().into_iter().find_map(|l| l.wav).and_then(|w| files::shape(&w, SHAPE_BARS)).unwrap_or_default();
                i.note = Some((format!("{} thrown away.", layer_name(n)), false));
            }
            Err(e) => i.note = Some((format!("It could not be thrown away: {e}"), true)),
        }
        let gone = self.idea.as_ref().is_some_and(|i| i.take.layer_count() == 0);
        if gone {
            // The last layer: the idea is gone (its sidecar with it).
            if let Some(s) = self.idea.as_ref().and_then(|i| i.take.sidecar.clone()) {
                let _ = std::fs::remove_file(s);
            }
            self.idea = None;
            self.message = Some(("Thrown away.".into(), false));
            self.screen = Screen::Main;
        } else {
            self.save_side(false);
        }
        self.rescan();
    }

    fn stop_playing(&mut self) {
        if self.playing.take().is_some()
            && let Some(r) = &self.rig
        {
            let _ = r.play_file(None);
        }
    }

    fn idea_for(&self, take: TakeFiles) -> Idea {
        let shape = take.all_layers().into_iter().find_map(|l| l.wav).and_then(|w| files::shape(&w, SHAPE_BARS)).unwrap_or_default();
        let name = words::display_name(&take.stem);
        let side = Sidecar::of(&take);
        Idea {
            side,
            made: Vec::new(),
            confirm_layer: None,
            orig_stem: take.stem.clone(),
            default_name: name.clone(),
            name,
            take,
            just_saved: false,
            notes: None,
            problem: None,
            shape,
            edited_at: None,
            name_error: None,
            confirm_throw: false,
            note: None,
        }
    }

    /// Opens screen 3 for an idea in the list.
    pub fn open_idea(&mut self, stem: &str) {
        self.apply_name();
        if let Some(t) = self.takes.iter().find(|t| t.stem == stem).cloned() {
            self.idea = Some(self.idea_for(t));
            self.screen = Screen::Idea;
        }
    }

    /// The name box changed (renamed after a pause, on Enter, or on leaving it).
    pub fn name_typed(&mut self) {
        if let Some(i) = &mut self.idea {
            i.edited_at = Some(self.now);
            i.name_error = None;
        }
    }

    /// Renames the idea's files to the name typed: the name the box started with (or nothing)
    /// keeps the Recorder's own; a name taken by another idea is refused and said.
    pub fn apply_name(&mut self) {
        let Some(i) = &mut self.idea else { return };
        i.edited_at = None;
        let typed = i.name.trim();
        let target = if typed.is_empty() || typed == i.default_name { i.orig_stem.clone() } else { file_safe(typed) };
        if target.is_empty() || target == i.take.stem {
            return;
        }
        // A take playing is stopped first (Windows cannot rename a file open for reading).
        if self.playing.as_ref().is_some_and(|p| p.stem == i.take.stem) {
            self.playing = None;
            if let Some(r) = &self.rig {
                let _ = r.play_file(None);
            }
        }
        match files::rename(&i.take, &target) {
            Ok(t) => i.take = t,
            Err(e) => i.name_error = Some(e.replace("take", "idea")),
        }
        self.scanned = None;
    }

    /// Plays a take (or stops it when it plays).
    pub fn play(&mut self, take: &TakeFiles) {
        let Some(rig) = &self.rig else { return };
        if self.playing.as_ref().is_some_and(|p| p.stem == take.stem) {
            self.playing = None;
            let _ = rig.play_file(None);
            return;
        }
        // One take alone, as recorded; an idea with layers (or one opened on screen 3, which may
        // loop) as its mix.
        let on_screen = self.screen == Screen::Idea && self.idea.as_ref().is_some_and(|i| i.take.stem == take.stem);
        let side = if on_screen { self.idea.as_ref().map(|i| i.side.clone()).unwrap_or_default() } else { Sidecar::of(take) };
        let looped = on_screen && side.looped;
        if take.layer_count() <= 1 && !looped && side.layers.get(&1).is_none_or(|m| *m == layers::LayerMix::default()) {
            match rig.play_file(take.wav.as_deref()) {
                Ok(()) => {
                    self.playing = Some(Playing { stem: take.stem.clone(), since: Instant::now(), seconds: take.seconds.unwrap_or(0.0), looped })
                }
                Err(e) => self.message = Some((format!("Cannot play it: {e}"), true)),
            }
            return;
        }
        match layers::mix(take, &side, rig.rate()) {
            Ok(m) => {
                let seconds = m.frames as f64 / f64::from(m.rate.max(1));
                let n = i64::try_from(m.frames).unwrap_or(i64::MAX);
                rig.play_mix(m.channels, looped.then_some(n));
                self.playing = Some(Playing { stem: take.stem.clone(), since: Instant::now(), seconds, looped });
            }
            Err(e) => self.message = Some((format!("Cannot play it: {e}"), true)),
        }
    }

    /// The take playing now, and how far (0..1; round and round when it loops).
    pub fn playing(&self) -> Option<(&str, f32)> {
        self.playing.as_ref().map(|p| {
            let t = p.since.elapsed().as_secs_f64();
            let secs = p.seconds.max(0.01);
            let at = if p.looped { t.rem_euclid(secs) / secs } else { t / secs };
            (p.stem.as_str(), at.clamp(0.0, 1.0) as f32)
        })
    }

    /// Throws the idea on screen 3 away (after it asked), back to the list.
    pub fn throw_away(&mut self) {
        let Some(i) = self.idea.take() else { return };
        if self.playing.as_ref().is_some_and(|p| p.stem == i.take.stem) {
            self.playing = None;
            if let Some(r) = &self.rig {
                let _ = r.play_file(None);
            }
        }
        match files::delete(&i.take) {
            Ok(()) => self.message = Some(("Thrown away.".into(), false)),
            Err(e) => self.message = Some((format!("It could not be thrown away: {e}"), true)),
        }
        self.screen = Screen::Main;
        self.rescan();
    }

    /// Opens the idea on screen 3 in FreeDAWm (a new session with it).
    pub fn open_in_daw(&mut self) {
        self.apply_name();
        let Some(i) = &mut self.idea else { return };
        let Some(daw) = &self.daw else {
            i.note = Some(("FreeDAWm isn't next to the Recorder, so it can't be opened from here.".into(), true));
            return;
        };
        i.note = Some(match std::process::Command::new(daw).args(daw_args(&i.take)).spawn() {
            Ok(_) => ("Opening it in FreeDAWm…".into(), false),
            Err(e) => (format!("FreeDAWm did not start: {e}"), true),
        });
    }

    /// Shows the idea's file in its folder.
    pub fn show_file(&mut self) {
        self.apply_name();
        let Some(i) = &mut self.idea else { return };
        let path = i.take.wav.clone().or_else(|| i.take.mid.clone()).unwrap_or_default();
        if let Err(e) = files::reveal(&path) {
            i.note = Some((e, true));
        }
    }

    /// Goes to `screen`: leaving screen 3 applies a name typed; the studio screen looks for
    /// studios while it is open (and only then); the settings read the device lists.
    pub fn go(&mut self, screen: Screen) {
        if self.screen == screen {
            return;
        }
        if self.screen == Screen::Idea {
            self.apply_name();
            // A mix playing round stops with its screen.
            if self.playing.as_ref().is_some_and(|p| p.looped) {
                self.stop_playing();
            }
        }
        if self.screen == Screen::Studio {
            self.link.stop_finding();
            self.finding_since = None;
        }
        match screen {
            Screen::Studio => {
                if !self.link.paired() {
                    self.link.find();
                    self.finding_since = Some(Instant::now());
                }
            }
            Screen::Settings => self.read_devices(),
            Screen::Main | Screen::Idea => {}
        }
        if screen == Screen::Main {
            self.message = self.message.take().filter(|(_, bad)| *bad);
        }
        self.screen = screen;
    }

    /// The studio box's Join: an invite joins over the internet, an address asks that studio, a
    /// PIN or code goes to the studio asked (or the one found here).
    pub fn studio_join(&mut self) {
        let text = self.studio_box.trim().to_string();
        if text.is_empty() {
            self.link.pair_status = "Paste the PIN or invite first.".into();
            return;
        }
        if text.to_lowercase().contains("freedawm-invite:") {
            self.link.join_invite(&text);
            self.studio_box.clear();
            return;
        }
        if self.link.wants_code() {
            self.link.code_edit = text;
            self.link.confirm();
            self.studio_box.clear();
            return;
        }
        if text.contains(':') || text.chars().filter(|c| *c == '.').count() == 3 {
            let addr = if text.contains(':') { text } else { format!("{text}:{}", soundcraft_net::config::DEFAULT_PORT) };
            self.link.choose("", &addr);
            self.link.pair();
            self.studio_box.clear();
            return;
        }
        if !text.chars().all(|c| c.is_ascii_digit() || c == ' ' || c == '-') {
            self.link.pair_status = "That isn't a PIN or an invite. An invite starts with freedawm-invite:".into();
            return;
        }
        let found = self.link.found();
        let asked = &self.link.cfg.follow;
        let asked_before = (!asked.booth_addr.is_empty()).then(|| (asked.booth_name.clone(), asked.booth_addr.clone()));
        let target = match found.as_slice() {
            [one] => Some((one.name.clone(), one.addr.clone())),
            [] => asked_before,
            many => many.iter().find(|f| f.name == asked.booth_name).map(|f| (f.name.clone(), f.addr.clone())),
        };
        match (target, found.len()) {
            (Some((name, addr)), _) => {
                self.link.pending_pin = Some(text.chars().filter(char::is_ascii_digit).collect());
                self.link.choose(&name, &addr);
                self.link.pair();
                self.studio_box.clear();
            }
            (None, 0) => {
                self.link.pair_status =
                    "No studio found on this network to use that PIN with. Check FreeDAWm is open there, or paste an invite.".into();
            }
            (None, _) => self.link.pair_status = "Press Join beside the studio first, then type its PIN.".into(),
        }
    }

    /// Join beside a studio found on this network.
    pub fn studio_join_found(&mut self, name: &str, addr: &str) {
        self.link.pending_pin = None;
        self.link.choose(name, addr);
        self.link.pair();
    }

    /// Leaves (forgets) the studio joined.
    pub fn leave_studio(&mut self) {
        if self.rig.as_ref().is_some_and(Rig::recording_for_booth) {
            return;
        }
        self.link.forget();
        self.link.pair_status.clear();
        if self.screen == Screen::Studio {
            self.link.find();
            self.finding_since = Some(Instant::now());
        }
    }

    /// Keys: Space records or stops (not while typing, nor on the studio and settings screens);
    /// Escape goes back to the main screen.
    fn keys(&mut self, ctx: &egui::Context) {
        let typing = ctx.text_edit_focused();
        let on_record_screen = self.recording() || matches!(self.screen, Screen::Main | Screen::Idea);
        if on_record_screen && !typing && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space)) {
            self.toggle_record();
        }
        if self.counting.is_some() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.counting = None;
        } else if !self.recording()
            && matches!(self.screen, Screen::Studio | Screen::Settings | Screen::Idea)
            && ctx.input(|i| i.key_pressed(egui::Key::Escape))
        {
            self.go(Screen::Main);
        }
    }

    /// The whole window (call once a frame).
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let now = ui.input(|i| i.time);
        self.step(now);
        self.keys(ui.ctx());
        if let Some(r) = ui.ctx().input(|i| i.viewport().outer_rect) {
            self.window = Some(([r.min.x, r.min.y], [r.width(), r.height()]));
        }
        self.draw(ui);
        let busy = self.recording() || self.playing.is_some() || self.rig.is_none() || self.link.busy() || self.counting.is_some() || self.measuring;
        ui.ctx().request_repaint_after(Duration::from_millis(if busy { 33 } else { 100 }));
    }
}

/// Why a take could not start, in plain words.
fn start_problem(e: &str, dir: &Path) -> String {
    let lower = e.to_lowercase();
    if lower.contains("nothing to record from") {
        "Nothing to record from: plug in a microphone or a keyboard.".into()
    } else if lower.contains("cannot use the folder") {
        format!("Your ideas folder can't be used ({}). Choose another in Settings.", dir.display())
    } else if lower.contains("space") || lower.contains("full") {
        "Your disk is full. Free some space, then press Record again.".into()
    } else {
        format!("Recording could not start: {e}")
    }
}
