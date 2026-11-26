//! The recorder's audio and MIDI side, built from the DAW's own parts:
//!
//! - the player ([`soundcraft_playback::Player`]) on the output device, which runs the clock the
//!   input is captured on, plays the booth's mix ([`soundcraft_playback::stream::StreamIn`]) and
//!   the takes' Play;
//! - the input recorder ([`soundcraft_playback::record::Recorder`]), whose writer streams each
//!   take to its WAV file as it records, its header rewritten every second, so a crash keeps
//!   the take;
//! - the MIDI input hub and ports ([`soundcraft_playback::midi_in`]), recorded by the audio thread
//!   while the take records;
//! - a small session ([`soundcraft_engine::Engine`]): one audio track on the chosen input
//!   (metered before its fader, the fader down so the input is never heard) and one MIDI track on
//!   all MIDI inputs, both armed. The booth glue reads it as the DAW's session.
//!
//! A take writes `<stem>.wav` (24-bit, at the session rate, which is the output device's) and,
//! when any MIDI came, `<stem>.mid` (120 bpm, notes at the real time they were played), made by
//! the DAW's own MIDI recording and MIDI file export.

use crate::layers::LayerRaw;
use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_engine::midi_record::{MidiTake, RecordedEvent, record_live_midi};
use soundcraft_model::TrackId;
use soundcraft_playback::Player;
use soundcraft_playback::devices::AudioConfig;
use soundcraft_playback::midi_in::{MidiHub, MidiInputs, RecordedMidi, TYPING_NAME, TYPING_SLOT};
use soundcraft_playback::record::{Recorder, TakeFormat, Trouble};
use soundcraft_time::{SampleRate, Samples};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// How the input is recorded: the channel (0-based) and whether the pair from it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputChoice {
    pub on: bool,
    pub channel: usize,
    pub stereo: bool,
}

impl InputChoice {
    /// The device channels a take records.
    pub fn channels(&self) -> Vec<usize> {
        if self.stereo { vec![self.channel, self.channel.saturating_add(1)] } else { vec![self.channel] }
    }

    /// The input's name as the DAW names it (`In 1`, `In 1-2`): how a booth lists it.
    pub fn name(&self) -> String {
        let n = self.channel.saturating_add(1);
        if self.stereo { format!("In {n}-{}", n.saturating_add(1)) } else { format!("In {n}") }
    }
}

/// A take recording now.
#[derive(Debug, Clone)]
struct Take {
    stem: String,
    folder: PathBuf,
    wav: Option<PathBuf>,
    began: Instant,
    /// Where the transport started (the take's first frame is there or just after).
    from: Samples,
    /// Recorded for the booth (its glue started it).
    for_booth: bool,
    input: String,
    /// A layer over an idea looping this many frames (see [`crate::layers`]).
    layer: Option<LayerJob>,
}

/// A layer take: the idea it is over, the layer number its file has while it records, the loop.
#[derive(Debug, Clone)]
struct LayerJob {
    idea: String,
    first: u32,
    loop_frames: i64,
}

/// What a take left on disk.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Saved {
    pub wav: Option<PathBuf>,
    pub mid: Option<PathBuf>,
    /// Notes in the MIDI file.
    pub notes: usize,
    pub seconds: f64,
    /// What went wrong, in words (the files written are still kept).
    pub problem: Option<String>,
    /// What kind of thing went wrong (with `problem`).
    pub cause: Option<Cause>,
    /// A layer take: what [`crate::layers::finish`] cuts into layers (its files are not final yet).
    pub layer: Option<LayerRaw>,
}

/// Why a take stopped early or lost part of itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// The audio input went away (unplugged).
    InputGone,
    /// The disk could not keep up, or is full.
    Disk,
    /// The audio output stopped (the take's clock).
    OutputGone,
    /// Another problem (a file not written).
    Other,
}

/// A booth take as the glue wants it once it stopped (see `crate::booth`).
#[derive(Debug, Clone, Default)]
pub struct BoothTake {
    pub midi: Option<(MidiTake, Vec<RecordedEvent>)>,
    /// The audio file, its timeline start, record offset, rate and input name.
    pub audio: Option<(PathBuf, Samples, Samples, u32, String)>,
}

/// The recorder's devices and its take.
pub struct Rig {
    pub player: Player,
    pub engine: Engine,
    hub: Arc<MidiHub>,
    pub midi: Option<MidiInputs>,
    input: Option<Recorder>,
    /// Why there is no input (shown in the window).
    pub input_error: Option<String>,
    pub choice: InputChoice,
    audio_track: TrackId,
    midi_track: TrackId,
    take: Option<Take>,
    heard: Vec<RecordedMidi>,
    /// Named, for the booth's live sketch.
    live: Vec<RecordedEvent>,
    /// The last booth take, for the glue.
    pub booth_take: Option<BoothTake>,
    /// The output device went away (said once).
    pub output_problem: Option<String>,
    last_from: Samples,
    /// Layers: a loopback round trip measured with **Line up** (and the feed latency it went
    /// through), and the manual nudge (frames; more places layers earlier).
    pub measured: Option<(i64, u32)>,
    pub nudge: i64,
}

/// The session the rig runs: one audio track on `choice`'s input and one MIDI track on all
/// MIDI inputs, both armed, at `rate`. Returns it and the two tracks.
fn session_for(rate: u32, choice: InputChoice) -> (Engine, TrackId, TrackId) {
    let mut e = Engine::default();
    if let Ok(r) = SampleRate::new(rate) {
        e.session_mut().sample_rate = r;
    }
    e.session_mut().name = "Recorder".into();
    // The input is metered before the fader, and the fader is down: the input is never heard.
    e.session_mut().edit.pre_fader_metering = true;
    let id = |v: Result<serde_json::Value, soundcraft_engine::EngineError>| v.ok().and_then(|v| v["tracks"][0].as_u64()).map(TrackId);
    let format = if choice.stereo { "stereo" } else { "mono" };
    let audio = id(e.execute("track.new", &json!({"kind": "audio", "name": "Audio", "format": format}))).unwrap_or(TrackId(0));
    let midi = id(e.execute("track.new", &json!({"kind": "midi", "name": "MIDI"}))).unwrap_or(TrackId(0));
    let _ = e.execute("track.input", &json!({"tracks": [audio.0], "input": choice.name()}));
    let _ = e.execute("track.input", &json!({"tracks": [midi.0], "input": "midi:all"}));
    let _ = e.execute("mix.volume", &json!({"tracks": [audio.0], "db": -144.0}));
    let armed: Vec<u64> = if choice.on { vec![audio.0, midi.0] } else { vec![midi.0] };
    let _ = e.execute("mix.record_arm", &json!({"tracks": armed, "value": true}));
    (e, audio, midi)
}

impl Rig {
    /// Opens the output device `audio` chooses (a silent clock without one), at its own rate,
    /// the input and the MIDI ports. Never fails: what is missing is said in `input_error`.
    pub fn open(audio: &AudioConfig, choice: InputChoice, midi_choices: BTreeMap<String, bool>) -> Rig {
        let want = audio.sample_rate.unwrap_or(48_000);
        let (engine, _, _) = session_for(want, choice);
        let mut player = Player::with_config(engine.session_arc(), audio);
        // Takes at the device's own rate: the session follows it when it runs at another.
        let rate = player.device_rate;
        if rate != want && SampleRate::new(rate).is_ok() {
            let cfg = AudioConfig { sample_rate: Some(rate), ..audio.clone() };
            let (e, _, _) = session_for(rate, choice);
            drop(player);
            player = Player::with_config(e.session_arc(), &cfg);
        }
        let hub = MidiHub::new();
        let midi = MidiInputs::system(Arc::clone(&hub), midi_choices);
        let mut rig = Rig::with_player(player, hub, midi, choice);
        rig.open_input(audio);
        rig
    }

    /// A rig on `player` (tests: [`Player::stalled`], an input from [`Recorder::synthetic`]).
    #[doc(hidden)]
    pub fn offline(player: Player, input: Option<Recorder>, midi: Option<MidiInputs>, choice: InputChoice) -> Rig {
        let hub = midi.as_ref().map_or_else(MidiHub::new, |m| Arc::clone(m.hub()));
        let mut rig = Rig::with_player(player, hub, midi, choice);
        rig.input = input;
        if rig.input.is_none() {
            rig.input_error = Some("No audio input".into());
        }
        rig
    }

    fn with_player(player: Player, hub: Arc<MidiHub>, midi: Option<MidiInputs>, choice: InputChoice) -> Rig {
        let rate = player.session_rate();
        let (engine, audio_track, midi_track) = session_for(rate, choice);
        player.update_session(engine.session_arc());
        player.set_midi(Arc::clone(&hub));
        Rig {
            player,
            engine,
            hub,
            midi,
            input: None,
            input_error: None,
            choice,
            audio_track,
            midi_track,
            take: None,
            heard: Vec::new(),
            live: Vec::new(),
            booth_take: None,
            output_problem: None,
            last_from: 0,
            measured: None,
            nudge: 0,
        }
    }

    fn open_input(&mut self, audio: &AudioConfig) {
        match self.player.open_input(audio) {
            Ok(r) => {
                self.input = Some(r);
                self.input_error = None;
            }
            Err(e) => {
                self.input = None;
                self.input_error = Some(format!("No audio input: {e}"));
            }
        }
    }

    /// The session rate (the takes').
    pub fn rate(&self) -> u32 {
        self.player.session_rate()
    }

    /// What a measured round trip belongs to: the audio system, the output and input devices,
    /// the buffer and the rate (as the DAW keys its record offsets).
    pub fn latency_key(&self) -> String {
        let p = &self.player;
        let device = format!("{} + {}", p.device_name, self.input_name().unwrap_or_default());
        soundcraft_playback::devices::latency_key(&p.system, &device, p.device_buffer, p.device_rate)
    }

    /// The input's channel count (0 without an input).
    pub fn input_channels(&self) -> usize {
        self.input.as_ref().map_or(0, |r| r.channels)
    }

    /// The input device's name.
    pub fn input_name(&self) -> Option<String> {
        self.input.as_ref().map(|r| r.device_name.clone())
    }

    /// Records the input `choice` says from the next take on.
    pub fn set_choice(&mut self, choice: InputChoice) {
        if choice == self.choice || self.recording() {
            return;
        }
        self.choice = choice;
        let (e, a, m) = session_for(self.rate(), choice);
        (self.engine, self.audio_track, self.midi_track) = (e, a, m);
        self.player.update_session(self.engine.session_arc());
    }

    /// The MIDI ports present now and whether each records.
    pub fn midi_ports(&self) -> Vec<(String, bool)> {
        self.midi.as_ref().map(|m| m.ports().into_iter().filter(|p| p.present).map(|p| (p.name, p.enabled)).collect()).unwrap_or_default()
    }

    pub fn set_midi_port(&mut self, name: &str, on: bool) {
        if let Some(m) = &mut self.midi {
            m.set_enabled(name, on);
        }
    }

    /// The ports' choices to save.
    pub fn midi_choices(&self) -> BTreeMap<String, bool> {
        self.midi.as_ref().map(|m| m.choices().clone()).unwrap_or_default()
    }

    /// Something to record from: an input (when audio is on) or a MIDI port.
    pub fn can_record(&self) -> bool {
        (self.choice.on && self.input.is_some()) || !self.midi_ports().is_empty()
    }

    pub fn recording(&self) -> bool {
        self.take.is_some()
    }

    /// The take recording now was started by the studio (the booth glue).
    pub fn recording_for_booth(&self) -> bool {
        self.take.as_ref().is_some_and(|t| t.for_booth)
    }

    /// Notes played so far in this take, counted from MIDI event `from` on (note-ons), and the
    /// next event to count from.
    pub fn notes_since(&self, from: usize) -> (usize, usize) {
        let new = self.live.get(from..).unwrap_or_default();
        (new.iter().filter(|e| e.msg[0] & 0xF0 == 0x90 && e.msg[2] > 0).count(), self.live.len())
    }

    /// The input's loudness over the last `bars` stretches of `blocks` live blocks each (0..1
    /// peaks, oldest first), while a take records audio: the moving bars.
    pub fn recent_peaks(&self, bars: usize, blocks: usize) -> Vec<f32> {
        let Some(r) = self.input.as_ref().filter(|_| self.take.is_some()) else { return Vec::new() };
        let Some(live) = r.live_take(self.audio_track) else { return Vec::new() };
        let have = live.first().map_or(0, Vec::len);
        let want = bars.saturating_mul(blocks.max(1));
        let from = have.saturating_sub(want);
        let mut out = Vec::with_capacity(bars);
        let mut at = from;
        while at < have {
            let end = at.saturating_add(blocks.max(1)).min(have);
            let peak = live.iter().filter_map(|c| c.get(at..end)).flatten().map(|(lo, hi)| lo.abs().max(hi.abs())).fold(0f32, f32::max);
            out.push(peak);
            at = end;
        }
        out
    }

    /// Seconds recorded so far.
    pub fn elapsed(&self) -> f64 {
        self.take.as_ref().map_or(0.0, |t| t.began.elapsed().as_secs_f64())
    }

    /// The input's level now (0..1 peak, every channel recorded), before the fader.
    pub fn level(&self) -> f32 {
        let m = self.player.meters();
        m.tracks.get(&self.audio_track).map_or(0.0, |s| s.peak[0].max(s.peak[1]))
    }

    /// Where the take recording now (or the last) started on the timeline.
    pub fn take_from(&self) -> Samples {
        self.take.as_ref().map_or(self.last_from, |t| t.from)
    }

    /// The name of the take recording now.
    pub fn current_stem(&self) -> Option<&str> {
        self.take.as_ref().map(|t| t.stem.as_str())
    }

    /// The session's audio track (the take's).
    pub fn audio_track(&self) -> TrackId {
        self.audio_track
    }

    /// Starts a take into `folder` as `<stem>.wav` / `<stem>.mid`, the transport rolling from
    /// `from` (a booth's position; 0 for an idea). `for_booth`: the booth glue started it (its
    /// files are handed to the glue at Stop).
    pub fn start(&mut self, folder: &Path, stem: &str, for_booth: bool, from: Samples) -> Result<(), String> {
        self.start_take(folder, stem, for_booth, from, None)
    }

    /// Starts layer `first` over idea `idea` (in `folder`): `mix` (the idea's unmuted layers,
    /// stereo at the session rate, `loop_frames` long) plays round and round from its start
    /// while the input and the keyboards record into `<idea> (layer first)`. Stop hands back the
    /// take to cut into layers ([`Saved::layer`]).
    pub fn start_layer(&mut self, folder: &Path, idea: &str, first: u32, mix: Vec<Vec<f32>>, loop_frames: i64) -> Result<(), String> {
        if loop_frames < 1 {
            return Err("This idea has nothing in it to play".into());
        }
        let stem = crate::layers::layer_stem(idea, first);
        let job = LayerJob { idea: idea.to_string(), first, loop_frames };
        self.start_take(folder, &stem, false, 0, Some(job))?;
        let sync = soundcraft_playback::PreviewSync { origin: 0, period: loop_frames };
        self.player.preview_synced(mix, self.rate(), 0, Some(sync));
        Ok(())
    }

    fn start_take(&mut self, folder: &Path, stem: &str, for_booth: bool, from: Samples, layer: Option<LayerJob>) -> Result<(), String> {
        if self.take.is_some() {
            return Err("already recording".into());
        }
        if !self.can_record() {
            return Err("Nothing to record from: no audio input and no MIDI input".into());
        }
        std::fs::create_dir_all(folder).map_err(|e| format!("Cannot use the folder {}: {e}", folder.display()))?;
        let mut wav = None;
        if self.choice.on
            && let Some(r) = &mut self.input
        {
            let chans = self.choice.channels();
            if chans.iter().any(|c| *c >= r.channels) {
                return Err(format!("{} is not on this input ({} channels)", self.choice.name(), r.channels));
            }
            let path = folder.join(format!("{stem}.wav"));
            let format = TakeFormat { depth: soundcraft_audio_io::BitDepth::Int24, ..TakeFormat::default() };
            r.arm_as(vec![(self.audio_track, chans, path.clone())], 0, &format).map_err(|e| format!("Cannot record: {e}"))?;
            wav = Some(path);
        }
        self.heard.clear();
        self.live.clear();
        // Anything played before the Record press is not this take's.
        let mut old = Vec::new();
        self.hub.take_recorded(&mut old);
        self.player.set_recording(true);
        let looped = layer.as_ref().map(|l| soundcraft_time::Range { start: 0, end: l.loop_frames });
        self.player.play(from, None, looped);
        self.last_from = from;
        self.take = Some(Take {
            stem: stem.to_string(),
            folder: folder.to_path_buf(),
            wav,
            began: Instant::now(),
            from,
            for_booth,
            input: self.choice.name(),
            layer,
        });
        Ok(())
    }

    /// Once a frame: the MIDI heard, the ports kept open, the devices watched. Returns a take
    /// that had to stop (a device went away, the disk is full), saved as far as it got.
    pub fn step(&mut self) -> Option<Saved> {
        if let Some(m) = &mut self.midi {
            m.tick(Instant::now());
        }
        self.collect();
        if self.output_problem.is_none()
            && let Some(t) = self.player.trouble()
        {
            self.output_problem = Some(format!("The audio output stopped ({t:?}): pick another output, or plug it back in and press Retry"));
            if self.take.is_some() {
                let mut saved = self.stop();
                saved.problem = saved.problem.or_else(|| Some("The audio output stopped. The take is saved up to there.".into()));
                saved.cause = Some(Cause::OutputGone);
                return Some(saved);
            }
        }
        let trouble = self.input.as_mut().filter(|_| self.take.is_some()).and_then(Recorder::trouble);
        if let Some(t) = trouble {
            let (why, cause) = match t {
                Trouble::Device(d) => (format!("The audio input stopped: {d}. The take is saved up to there."), Cause::InputGone),
                Trouble::Disk(d) => (format!("The disk could not keep up ({d}). The take is saved up to there."), Cause::Disk),
            };
            let mut saved = self.stop();
            if cause == Cause::InputGone {
                self.input = None;
                self.input_error = Some("The audio input was disconnected: plug it back in and press Retry".into());
            }
            saved.problem = Some(why);
            saved.cause = Some(cause);
            return Some(saved);
        }
        None
    }

    fn collect(&mut self) {
        let mut got = Vec::new();
        self.hub.take_recorded(&mut got);
        if got.is_empty() || self.take.is_none() {
            return;
        }
        let names = self.slot_names();
        self.live.extend(got.iter().map(|r| named(&names, r)));
        self.heard.extend(got);
    }

    fn slot_names(&self) -> Vec<(usize, String)> {
        let mut v: Vec<(usize, String)> = self.midi.as_ref().map(|m| m.ports().into_iter().map(|p| (p.slot, p.name)).collect()).unwrap_or_default();
        v.push((TYPING_SLOT, TYPING_NAME.to_string()));
        v
    }

    /// The MIDI heard in this take from event `from` on (the booth's live sketch).
    pub fn live_since(&self, from: usize) -> Vec<RecordedEvent> {
        self.live.get(from..).map(<[RecordedEvent]>::to_vec).unwrap_or_default()
    }

    /// The audio take recording now (per channel, min/max of each `LIVE_BLOCK` frames), from
    /// block `from` on, and its first frame's timeline position: the booth's live level.
    pub fn live_audio(&self, from: usize) -> Option<(TrackId, String, usize, Vec<f32>, Samples)> {
        let take = self.take.as_ref()?;
        let r = self.input.as_ref()?;
        let live = r.live_take(self.audio_track)?;
        let have = live.first().map_or(0, Vec::len);
        let begin = from.min(have);
        let peaks = (begin..have).map(|i| live.iter().filter_map(|c| c.get(i)).map(|(lo, hi)| lo.abs().max(hi.abs())).fold(0f32, f32::max)).collect();
        Some((self.audio_track, take.input.clone(), begin, peaks, r.start_pos().unwrap_or(take.from)))
    }

    /// The record offset of the take now: the input's and output's latencies and the feed's and
    /// mix's at its first frame (as the DAW computes it before a loopback measurement).
    pub fn record_offset(&self) -> Samples {
        let mut l = self.player.reported_latency();
        if let Some(r) = &self.input {
            l.feed = r.start_feed_latency().unwrap_or(l.feed);
            l.mix = r.start_mix_latency().unwrap_or(l.mix);
        }
        soundcraft_playback::measure::record_latency(None, 0, l)
    }

    /// The record offset of a layer take: as [`Rig::record_offset`], the round trip **Line up**
    /// measured replacing the reported one, plus the nudge.
    pub fn layer_offset(&self) -> Samples {
        let mut l = self.player.reported_latency();
        if let Some(r) = &self.input {
            l.feed = r.start_feed_latency().unwrap_or(l.feed);
            l.mix = r.start_mix_latency().unwrap_or(l.mix);
        }
        soundcraft_playback::measure::record_latency(self.measured, self.nudge, l)
    }

    /// A layer recording: the idea it is over and the layer number its file has.
    pub fn layering(&self) -> Option<(&str, u32)> {
        self.take.as_ref().and_then(|t| t.layer.as_ref()).map(|l| (l.idea.as_str(), l.first))
    }

    /// While a layer records or a mix plays round: how far round the loop it is (0..1) and which
    /// time round (0 for the first).
    pub fn loop_at(&self, loop_frames: i64) -> (f32, u32) {
        let n = loop_frames.max(1);
        let pos = self.player.position().rem_euclid(n);
        ((pos as f64 / n as f64) as f32, u32::from(self.player.loop_pass()))
    }

    /// How much earlier MIDI was played than it was heard (the output's and mix's latency).
    pub fn midi_latency(&self) -> Samples {
        let l = self.player.reported_latency();
        i64::from(l.output) + i64::from(l.mix)
    }

    /// Stops the take: its WAV is finished, its MIDI written as `<stem>.mid` when any came.
    pub fn stop(&mut self) -> Saved {
        let end = self.player.position();
        self.player.stop();
        self.player.set_recording(false);
        let Some(take) = self.take.take() else { return Saved::default() };
        if let Some(job) = take.layer.clone() {
            // The loop stops with the take.
            self.player.preview(Vec::new(), self.rate());
            return self.stop_layer(&take, job);
        }
        self.collect();
        let (mut files, mut problem, start) = (Vec::new(), None, take.from);
        let start = match &mut self.input {
            Some(r) if take.wav.is_some() => {
                (files, problem) = r.finish();
                r.start_pos().unwrap_or(start)
            }
            _ => start,
        };
        let rate = self.rate();
        let wav = files.into_iter().map(|(_, p)| p).next();
        let events: Vec<RecordedEvent> = std::mem::take(&mut self.live);
        self.heard.clear();
        let midi_take = MidiTake { start: take.from, end: end.max(take.from), looped: None, punch: None, latency: self.midi_latency() };
        let mut saved = Saved { wav: wav.clone(), seconds: take.began.elapsed().as_secs_f64(), problem, ..Saved::default() };
        if let Some((bytes, notes)) = midi_file(rate, &midi_take, &events) {
            let path = take.folder.join(format!("{}.mid", take.stem));
            match std::fs::write(&path, bytes) {
                Ok(()) => (saved.mid, saved.notes) = (Some(path), notes),
                Err(e) => saved.problem = saved.problem.or(Some(format!("The MIDI file could not be written: {e}"))),
            }
        }
        if take.for_booth {
            let latency = self.record_offset();
            self.booth_take = Some(BoothTake { midi: Some((midi_take, events)), audio: wav.map(|p| (p, start, latency, rate, take.input.clone())) });
        }
        saved
    }

    /// A layer take stopped: its WAV finished, handed back with its MIDI to be cut into layers.
    fn stop_layer(&mut self, take: &Take, job: LayerJob) -> Saved {
        self.collect();
        let (mut wav, mut problem, mut start) = (None, None, 0);
        if let Some(r) = &mut self.input
            && take.wav.is_some()
        {
            let files;
            (files, problem) = r.finish();
            wav = files.into_iter().map(|(_, p)| p).next();
            start = r.start_pos().unwrap_or(0);
        }
        let offset = self.layer_offset();
        let events: Vec<RecordedEvent> = std::mem::take(&mut self.live);
        self.heard.clear();
        let raw = LayerRaw {
            idea: job.idea,
            first: job.first,
            folder: take.folder.clone(),
            wav: wav.clone().or_else(|| take.wav.clone()),
            start,
            offset,
            loop_frames: job.loop_frames,
            rate: self.rate(),
            events,
            midi_latency: self.midi_latency(),
        };
        Saved { wav, seconds: take.began.elapsed().as_secs_f64(), problem, layer: Some(raw), ..Saved::default() }
    }

    /// Plays an idea's mix (stereo at the session rate) through the output now: round and round
    /// from its start (the transport looping `loop_frames`) when given, else once.
    pub fn play_mix(&self, mix: Vec<Vec<f32>>, loop_frames: Option<i64>) {
        match loop_frames.filter(|n| *n > 0) {
            Some(n) if !self.recording() => {
                self.player.play(0, None, Some(soundcraft_time::Range { start: 0, end: n }));
                self.player.preview_synced(mix, self.rate(), 0, Some(soundcraft_playback::PreviewSync { origin: 0, period: n }));
            }
            _ => self.player.preview(mix, self.rate()),
        }
    }

    /// Plays a take's WAV through the output now (empty: stops it, and a mix playing round).
    pub fn play_file(&self, path: Option<&Path>) -> Result<(), String> {
        let Some(path) = path else {
            self.player.preview(Vec::new(), self.rate());
            if !self.recording() && self.player.is_playing() {
                self.player.stop();
            }
            return Ok(());
        };
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let (_, buf) = soundcraft_audio_io::decode(&bytes, Some("wav")).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut ch = buf.channels;
        if let [mono] = ch.as_slice() {
            let m = mono.clone();
            ch.push(m);
        }
        self.player.preview(ch, buf.sample_rate);
        Ok(())
    }
}

fn named(names: &[(usize, String)], r: &RecordedMidi) -> RecordedEvent {
    let port = names.iter().find(|(s, _)| *s == usize::from(r.port)).map(|(_, n)| n.clone()).unwrap_or_default();
    RecordedEvent { at: r.at, port, msg: r.msg, pass: r.pass, sysex: r.sysex.as_deref().map(<[u8]>::to_vec).unwrap_or_default() }
}

/// The take's MIDI as a Standard MIDI File at 120 bpm (each note at the real time it was played,
/// from the take's start), made the way the DAW records and exports MIDI; and its note count.
/// `None` when no note or controller came.
pub fn midi_file(rate: u32, take: &MidiTake, events: &[RecordedEvent]) -> Option<(Vec<u8>, usize)> {
    if events.is_empty() {
        return None;
    }
    let (mut e, _, midi) = session_for(rate, InputChoice { on: false, channel: 0, stereo: false });
    if let Ok(t) = soundcraft_time::TempoMap::new(120.0, 4, 4) {
        e.session_mut().tempo = t;
    }
    // The take starts the file: the transport rolled from the timeline's start.
    let take = MidiTake { start: take.start, end: take.end, ..*take };
    let clips = record_live_midi(&mut e, &take, events);
    if clips.is_empty() {
        return None;
    }
    // The file's track has no name of its own: a program opening it names it after the file
    // (which the Recorder renames with the idea).
    if let Some(t) = e.session_mut().track_mut(midi) {
        t.name.clear();
    }
    let notes = e.session().track(midi).map(|t| t.clips().iter().filter_map(|c| c.midi_playing()).map(|m| m.notes.len()).sum::<usize>()).unwrap_or(0);
    let bytes = soundcraft_engine::io::export_midi(&e, Some(&[midi])).ok()?;
    Some((bytes, notes))
}
