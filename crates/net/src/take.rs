//! Takes recorded on a remote PC, as they travel to the booth and land there.
//!
//! A [`TakeEnvelope`] is generic over what was recorded ([`TakePayload`]): today MIDI (the
//! events, each at its remote take-clock time), later audio (a file and its rate) through the
//! same envelope, sync points and anchor. The booth maps each time onto its own take clock
//! ([`crate::sync::ClockMap`]), then onto its timeline through the [`BoothAnchor`] the booth sent
//! when the take began (where its transport started, its loop and punch), so a take that
//! arrives hours later, or after the booth restarted, still lands where it was played.

use serde::{Deserialize, Serialize};
use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_engine::midi_record::{MidiTake, RecordedEvent, record_midi_on, tracks_on_port};
use soundcraft_model::{ClipId, TrackId};
use soundcraft_time::Range;

use crate::sync::{ClockMap, SyncPoint};

/// The booth input name of a remote recorder's port: `Remote: <PC> › <port>`.
pub fn remote_port_name(pc: &str, port: &str) -> String {
    format!("Remote: {pc} › {port}")
}

/// Where the booth's take sits on its timeline, in booth seconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoothAnchor {
    /// Where the booth's transport started (its take clock's zero).
    pub play_from: f64,
    /// Where its recording started and stopped (`end` once it stopped).
    pub record_start: f64,
    #[serde(default)]
    pub end: Option<f64>,
    /// Loop recording: the loop, `[start, end]`.
    #[serde(default)]
    pub looped: Option<[f64; 2]>,
    /// Punch recording: the range kept.
    #[serde(default)]
    pub punch: Option<[f64; 2]>,
}

impl BoothAnchor {
    /// The booth timeline position (seconds) and loop pass at booth take-clock time `rolled`.
    pub fn place(&self, rolled: f64) -> (f64, u16) {
        match self.looped {
            Some([s, e]) if e - s > 0.001 => {
                let len = e - s;
                let pass = (rolled / len).floor().max(0.0);
                (s + rolled - pass * len, pass.min(f64::from(u16::MAX)) as u16)
            }
            _ => (self.play_from + rolled, 0),
        }
    }
}

/// One MIDI message on the remote's take clock (seconds, the time it was played), from `port`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireEvent {
    pub t: f64,
    pub port: String,
    pub msg: [u8; 3],
}

/// What a take holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TakePayload {
    Midi {
        events: Vec<WireEvent>,
    },
    /// One armed audio track's take: recorded from `input` (the booth offers
    /// `Remote: <PC> › <input>`) at `rate`, its first frame at remote take-clock time `t0`. The
    /// transfer file (`file`, FLAC at the booth project's `bits`, or float WAV for 32) travels
    /// first in chunks (`take.offer`, `take.chunk`); this envelope follows once it has arrived.
    Audio(AudioTake),
}

/// A remote audio take (see [`TakePayload::Audio`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AudioTake {
    pub input: String,
    pub rate: u32,
    pub channels: u16,
    /// 16, 24 or 32 (float): the booth project's bit depth when the take began.
    #[serde(default)]
    pub bits: u16,
    pub t0: f64,
    /// The transfer file's name (`<id>.flac`), its size and hash ([`crate::audio::hash`]); size
    /// 0 until it has been made from the local take.
    pub file: String,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub frames: u64,
    /// Where clips start (remote take-clock seconds).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<f64>,
    /// The mix stream's map (remote take clock → booth take clock at the sample the performer
    /// heard), when the performer recorded against the stream: used instead of the sync points.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub heard: Vec<SyncPoint>,
    /// On the remote only (never needed by the booth): the local take file and the transfer
    /// file's folder.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dir: String,
}

impl AudioTake {
    /// The map from this take's remote clock to the booth's.
    pub fn clock_map(&self, sync: &[SyncPoint]) -> Option<ClockMap> {
        if self.heard.is_empty() { ClockMap::new(sync) } else { ClockMap::exact(&self.heard) }
    }
}

/// A finished take on its way from a remote recorder to the booth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TakeEnvelope {
    /// Unique per remote PC and booth take: the booth lands each id once.
    pub id: String,
    /// The booth's take this one belongs to.
    pub booth_take: u64,
    /// The remote PC's name.
    pub pc: String,
    pub anchor: BoothAnchor,
    pub sync: Vec<SyncPoint>,
    /// The remote joined after the booth started (it was unreachable at Record).
    #[serde(default)]
    pub partial: bool,
    pub payload: TakePayload,
}

impl TakeEnvelope {
    /// How many notes (note-ons) a MIDI take holds.
    pub fn notes(&self) -> usize {
        match &self.payload {
            TakePayload::Midi { events } => events.iter().filter(|e| e.msg[0] & 0xF0 == 0x90 && e.msg[2] > 0).count(),
            TakePayload::Audio(_) => 0,
        }
    }

    /// The audio take it carries.
    pub fn audio(&self) -> Option<&AudioTake> {
        match &self.payload {
            TakePayload::Audio(a) => Some(a),
            TakePayload::Midi { .. } => None,
        }
    }
}

/// What landing a take did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Landed {
    pub clips: Vec<ClipId>,
    /// Tracks made for ports no booth track listened to.
    pub new_tracks: Vec<TrackId>,
}

/// Lands `env` in the booth session: each event mapped onto the booth timeline and placed on the
/// tracks whose input is that port (`Remote: <PC> › <port>`, armed or not), as local MIDI
/// recording places a take (same playlist and loop rules, one "Record" undo step). A port no
/// track listens to gets a new MIDI track on that input, so a take is never dropped.
pub fn land(e: &mut Engine, env: &TakeEnvelope) -> Result<Landed, String> {
    let TakePayload::Midi { events } = &env.payload else {
        return Err("an audio take lands through land_audio".into());
    };
    let map = ClockMap::new(&env.sync);
    let rate = f64::from(e.session().sample_rate.hz());
    let to_samples = |secs: f64| (secs * rate).round().clamp(-9.0e15, 9.0e15) as i64;
    let mut out = Landed::default();
    let mut ports: Vec<String> = Vec::new();
    for ev in events {
        let name = remote_port_name(&env.pc, &ev.port);
        if !ports.contains(&name) {
            ports.push(name);
        }
    }
    let mut tracks: Vec<TrackId> = Vec::new();
    for port in &ports {
        let mut on = tracks_on_port(e.session(), port);
        if on.is_empty() {
            let made = e
                .execute("track.new", &json!({"kind": "midi", "name": port.trim_start_matches("Remote: ").replace(" › ", " ")}))
                .map_err(|e| e.to_string())?;
            if let Some(id) = made["tracks"].get(0).and_then(serde_json::Value::as_u64) {
                e.execute("track.input", &json!({"tracks": [id], "input": format!("midi:{port}")})).map_err(|e| e.to_string())?;
                out.new_tracks.push(TrackId(id));
                on.push(TrackId(id));
            }
        }
        for t in on {
            if !tracks.contains(&t) {
                tracks.push(t);
            }
        }
    }
    let placed: Vec<RecordedEvent> = events
        .iter()
        .map(|ev| {
            let booth = map.as_ref().map_or(ev.t, |m| m.map(ev.t));
            let (at, pass) = env.anchor.place(booth);
            RecordedEvent { at: to_samples(at), port: remote_port_name(&env.pc, &ev.port), msg: ev.msg, pass }
        })
        .collect();
    let a = &env.anchor;
    let last = placed.iter().map(|x| x.at).max().unwrap_or(0);
    let start = to_samples(a.record_start);
    let end = a.end.map_or(last.saturating_add(1), to_samples).max(start);
    let take = MidiTake {
        start,
        end,
        looped: a.looped.map(|[s, e]| Range::new(to_samples(s), to_samples(e))),
        punch: a.punch.map(|[s, e]| Range::new(to_samples(s), to_samples(e))),
        latency: 0,
    };
    out.clips = record_midi_on(e, &tracks, &take, &placed);
    Ok(out)
}

/// The booth tracks taking audio from `port` (`Remote: <PC> › <input>`).
pub fn tracks_on_audio_port(s: &soundcraft_model::Session, port: &str) -> Vec<TrackId> {
    s.tracks
        .iter()
        .filter(|t| t.kind == soundcraft_model::TrackKind::Audio && matches!(&t.mixer.input, soundcraft_model::Route::Hardware(h) if h == port))
        .map(|t| t.id)
        .collect()
}

/// Lands an audio take whose file is ready in the booth project (`wav`, at the session rate,
/// already on the booth's take clock with its first frame at booth take-clock sample `first`,
/// see [`crate::audio::align`]): on the tracks whose input is its port (armed or not), placed as
/// a local take is (the booth's loop and punch, same playlist rules, one "Record" undo step). A
/// port no track listens to gets a new audio track on that input.
pub fn land_audio(e: &mut Engine, env: &TakeEnvelope, wav: &std::path::Path, first: i64) -> Result<Landed, String> {
    let Some(a) = env.audio() else { return Err("not an audio take".into()) };
    let port = remote_port_name(&env.pc, &a.input);
    let mut out = Landed::default();
    let mut tracks = tracks_on_audio_port(e.session(), &port);
    if tracks.is_empty() {
        let format = if a.channels >= 2 { "stereo" } else { "mono" };
        let name = format!("{} {}", env.pc, a.input);
        let made = e.execute("track.new", &json!({"kind": "audio", "format": format, "name": name})).map_err(|e| e.to_string())?;
        if let Some(id) = made["tracks"].get(0).and_then(serde_json::Value::as_u64) {
            e.execute("track.input", &json!({"tracks": [id], "input": port})).map_err(|e| e.to_string())?;
            out.new_tracks.push(TrackId(id));
            tracks.push(TrackId(id));
        }
    }
    let rate = f64::from(e.session().sample_rate.hz());
    let to_samples = |secs: f64| (secs * rate).round().clamp(-9.0e15, 9.0e15) as i64;
    let anc = &env.anchor;
    // The take clock counts from where the booth's transport started (a loop's start when loop
    // recording); frame 0 sits `first` samples after it, so the "latency" skipping it is -first.
    let base = anc.looped.map_or(anc.play_from, |[s, _]| s);
    let plan = soundcraft_engine::io::Recording {
        start: to_samples(base),
        pass: anc.looped.map(|[s, e]| to_samples(e).saturating_sub(to_samples(s))).filter(|p| *p > 0),
        // What the booth recorded: its punch, else from its record start to where it stopped (a
        // performer hearing the stream late has more on either side).
        punch: anc
            .punch
            .or_else(|| anc.looped.is_none().then_some(()).and(anc.end).map(|e| [anc.record_start, e]))
            .map(|[s, e]| Range::new(to_samples(s), to_samples(e))),
        rate: e.session().sample_rate.hz(),
        takes: Vec::new(),
        latency: first.saturating_neg(),
    };
    let takes: Vec<(TrackId, std::path::PathBuf)> = tracks.iter().map(|t| (*t, wav.to_path_buf())).collect();
    out.clips = soundcraft_engine::io::place_recording(e, &plan, takes);
    if out.clips.is_empty() {
        return Err(format!("{}: the take could not be placed", env.pc));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_time::TICKS_PER_QUARTER;

    fn env(events: Vec<WireEvent>, sync: Vec<SyncPoint>, anchor: BoothAnchor) -> TakeEnvelope {
        TakeEnvelope {
            id: "piano-1".into(),
            booth_take: 1,
            pc: "Piano PC".into(),
            anchor,
            sync,
            partial: false,
            payload: TakePayload::Midi { events },
        }
    }

    fn ev(t: f64, msg: [u8; 3]) -> WireEvent {
        WireEvent { t, port: "Digital Piano".into(), msg }
    }

    #[test]
    fn a_take_lands_on_the_track_listening_to_its_port() {
        let mut e = Engine::default();
        let id = e.execute("track.new", &json!({"kind": "instrument", "name": "Piano MIDI"})).unwrap()["tracks"][0].as_u64().unwrap();
        e.execute("track.input", &json!({"tracks": [id], "input": "midi:Remote: Piano PC › Digital Piano"})).unwrap();
        // The remote's clock is 3 s behind the booth's.
        let sync = vec![SyncPoint { local: 0.0, booth: 3.0, rtt: 0.001 }, SyncPoint { local: 100.0, booth: 103.0, rtt: 0.001 }];
        let anchor = BoothAnchor { play_from: 1.0, record_start: 1.0, end: Some(10.0), looped: None, punch: None };
        // Played at remote 0.5 s: booth clock 3.5 s, timeline 4.5 s (= 9 quarters at 120 bpm).
        let l = land(&mut e, &env(vec![ev(0.5, [0x90, 60, 100]), ev(1.0, [0x80, 60, 0])], sync, anchor)).unwrap();
        assert_eq!(l.clips.len(), 1);
        assert!(l.new_tracks.is_empty());
        let (tid, clip) = e.session().find_clip(l.clips[0]).map(|(t, c)| (t, c.clone())).unwrap();
        assert_eq!(tid.0, id);
        assert_eq!(clip.start, 48_000);
        let n = clip.midi_playing().unwrap().notes[0];
        assert_eq!(n.start, 7 * TICKS_PER_QUARTER, "4.5 s - 1 s clip start = 3.5 s = 7 quarters");
        assert_eq!(e.undo_label(), Some("Record"));
    }

    #[test]
    fn a_port_nobody_listens_to_gets_its_own_track() {
        let mut e = Engine::default();
        let anchor = BoothAnchor { play_from: 0.0, record_start: 0.0, end: None, looped: None, punch: None };
        let l = land(&mut e, &env(vec![ev(0.25, [0x90, 64, 90])], vec![], anchor)).unwrap();
        assert_eq!(l.new_tracks.len(), 1);
        let t = e.session().track(l.new_tracks[0]).unwrap();
        assert_eq!(t.name, "Piano PC Digital Piano");
        assert_eq!(t.clips().len(), 1);
    }

    #[test]
    fn loop_passes_are_placed_by_the_booth_loop() {
        let a = BoothAnchor { play_from: 2.0, record_start: 2.0, end: None, looped: Some([2.0, 6.0]), punch: None };
        assert_eq!(a.place(1.0), (3.0, 0));
        assert_eq!(a.place(9.0), (3.0, 2));
        let lin = BoothAnchor { looped: None, ..a };
        assert_eq!(lin.place(9.0), (11.0, 0));
    }

    fn audio_env(anchor: BoothAnchor) -> TakeEnvelope {
        TakeEnvelope {
            id: "Guitar PC-1-a1".into(),
            booth_take: 1,
            pc: "Guitar PC".into(),
            anchor,
            sync: vec![],
            partial: false,
            payload: TakePayload::Audio(AudioTake { input: "In 1".into(), rate: 48_000, channels: 1, bits: 24, ..AudioTake::default() }),
        }
    }

    fn wav(dir: &std::path::Path, frames: usize) -> std::path::PathBuf {
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![(0..frames).map(|i| (i % 100) as f32 / 200.0).collect()] };
        let path = dir.join("Audio Files").join("Guitar PC In 1_01.wav");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, crate::audio::encode_landed(&buf, 24).unwrap()).unwrap();
        path
    }

    #[test]
    fn an_audio_take_lands_on_the_track_on_its_remote_input() {
        let dir = std::env::temp_dir().join(format!("freedawm-land-audio-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Engine::default();
        let gtr = e.execute("track.new", &json!({"kind": "audio", "name": "Guitar"})).unwrap()["tracks"][0].as_u64().unwrap();
        e.execute("track.input", &json!({"tracks": [gtr], "input": "Remote: Guitar PC › In 1"})).unwrap();
        assert_eq!(tracks_on_audio_port(e.session(), "Remote: Guitar PC › In 1"), vec![TrackId(gtr)]);
        let path = wav(&dir, 4_800);
        let anchor = BoothAnchor { play_from: 1.0, record_start: 1.0, end: Some(5.0), looped: None, punch: None };
        // Its first frame 0.5 s after the booth's transport started at 1 s.
        let l = land_audio(&mut e, &audio_env(anchor), &path, 24_000).unwrap();
        assert!(l.new_tracks.is_empty());
        let (tid, clip) = e.session().find_clip(l.clips[0]).map(|(t, c)| (t, c.clone())).unwrap();
        assert_eq!(tid.0, gtr);
        assert_eq!((clip.start, clip.length), (72_000, 4_800), "sample-exact");
        assert_eq!(e.undo_label(), Some("Record"));
        // Punch: only the kept range.
        let punch = BoothAnchor { punch: Some([1.52, 1.55]), ..anchor };
        let l = land_audio(&mut e, &audio_env(punch), &path, 24_000).unwrap();
        let clip = e.session().find_clip(l.clips[0]).unwrap().1.clone();
        assert_eq!((clip.start, clip.length), (72_960, 1_440));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_audio_port_nobody_listens_to_gets_its_own_track() {
        let dir = std::env::temp_dir().join(format!("freedawm-land-audio2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Engine::default();
        let path = wav(&dir, 480);
        let anchor = BoothAnchor { play_from: 0.0, record_start: 0.0, end: None, looped: Some([2.0, 2.005]), punch: None };
        // Loop passes of 240 samples: two passes, the second on a new playlist.
        let l = land_audio(&mut e, &audio_env(anchor), &path, 0).unwrap();
        assert_eq!(l.new_tracks.len(), 1);
        let t = e.session().track(l.new_tracks[0]).unwrap();
        assert_eq!(t.name, "Guitar PC In 1");
        assert_eq!(t.mixer.input, soundcraft_model::Route::Hardware("Remote: Guitar PC › In 1".into()));
        assert_eq!(l.clips.len(), 2);
        assert_eq!(t.playlists.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
