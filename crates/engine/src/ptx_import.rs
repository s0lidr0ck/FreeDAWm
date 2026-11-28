//! File › Import › Pro Tools Session... (`file.import_ptx`): a Pro Tools session (`.ptx`, Pro
//! Tools 10 and later) as new tracks in the open session, one undo step, as AAF import does.
//!
//! The file is read by `soundcraft-ptx` (the reader of ptx-session-kit, vendored and hardened)
//! into its decoded document; this module, FreeDAWm's own, maps that onto the session: tracks
//! (audio, MIDI, instrument, aux, master, VCA, folders) with their names, comments, order, colour,
//! hidden/inactive state, folder nesting, mute, solo, volume, pan, sends and buses, groups; audio
//! clips with positions, source offsets, sync points, fades, crossfades, clip gain and its
//! envelope, mute, loops (as Clip › Loop copies), clip groups and alternate playlists; MIDI clips
//! with notes, controllers, pitch bend, pressure, program and bank changes and SysEx; the tempo
//! and meter maps, key signatures, markers and selections, the timecode start and frame rate;
//! volume, pan, mute and send automation; and the stock plug-ins that have a clear FreeDAWm
//! equivalent (EQ3 1-Band and 7-Band as CleanQ, Dyn3 Compressor/Limiter as Compressor/Limiter,
//! Trim as Trim). Everything else is named in the report, which goes in the Notes tab: what came
//! across exactly, what was approximated and what was left out.
//!
//! Every file is read through [`crate::io`]'s checked helpers. Audio is looked for in the
//! session's `Audio Files` folder and beside the `.ptx`, and imported as File › Import › Audio
//! would (referred to where it is unless the session copies imported audio); a file that is not
//! there or cannot be read comes in offline under its name, as missing audio does when a session
//! opens (relink it with Clip › Relink Audio...).
//!
//! Positions: a session with no clips yet takes the Pro Tools session's tempo map, meters, key
//! signatures, timecode start and frame rate, so everything sits where it sat; a session with
//! clips keeps its own and the Pro Tools session is placed by timecode against it (bar-based MIDI
//! keeps its time in seconds, not its bars). `at` places the session start anywhere instead.

use crate::{Engine, EngineError, Result};
use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_dsp::plugins::fs as stock;
use soundcraft_midi::{CtrlEvent, CtrlKind, CtrlShape, Note, Sequence, SysEx};
use soundcraft_model::{
    AutoParam, ChannelFormat, Clip, Fade, FadeShape, Group, GroupId, Insert, MarkerKind, Playlist, Route, SendSlot, Session, Source, SourceId,
    TrackId, TrackKind,
};
use soundcraft_ptx::PtxDocument;
use soundcraft_ptx::decode::clips::{AudioPlacement, ClipLoop, FadeObject, MidiPlacement, Region};
use soundcraft_ptx::decode::midi::MidiChunk;
use soundcraft_ptx::decode::mixer::Strip;
use soundcraft_ptx::decode::plugins::Insert as PInsert;
use soundcraft_ptx::decode::tracks::{Track as PTrack, TrackPlaylists};
use soundcraft_time::{FrameRate, SampleRate, Samples, TempoMap, Timecode};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The largest `.ptx` read (it holds no audio).
pub const MAX_PTX_BYTES: u64 = soundcraft_ptx::MAX_PTX_BYTES as u64;
/// The largest audio file read.
const MAX_MEDIA_BYTES: u64 = 4 << 30;
/// The most clips one import makes (a loop's copies count).
const MAX_CLIPS: usize = 100_000;
/// The most copies one clip loop makes.
const MAX_LOOP_COPIES: u64 = 1_000;
/// Positions and lengths are kept below this (about 11 years at 48 kHz) so later sums cannot
/// overflow.
const MAX_POS: Samples = 1 << 44;
/// Pro Tools ticks per FreeDAWm tick (960 000 to the quarter note against 960).
const PT_TICKS_PER_TICK: f64 = 1000.0;

/// Reads the `.ptx` at `path` and imports it ([`import_bytes`]); audio is looked for beside it.
pub fn import_path(e: &mut Engine, path: &str, at: Option<Samples>) -> Result<Value> {
    let bytes = crate::io::read_file_max(path, MAX_PTX_BYTES)?;
    let p = Path::new(path);
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string());
    import_bytes(e, &bytes, &name, p.parent(), at)
}

/// Imports a `.ptx` held in memory; its audio is looked for in `dir`'s `Audio Files` and `dir`.
pub fn import_bytes(e: &mut Engine, bytes: &[u8], file: &str, dir: Option<&Path>, at: Option<Samples>) -> Result<Value> {
    let doc = soundcraft_ptx::decode_bytes(file, bytes).map_err(|err| EngineError::Io(format!("{file}: {err}")))?;
    if doc.tracks.is_empty() {
        return Err(EngineError::Io(format!("{file}: no tracks found (damaged, or not a Pro Tools 10 or later session)")));
    }
    let copy = e.copy_imported_audio;
    let s = e.session_mut();
    let mut p = Placer::new(s, &doc, dir, copy);
    p.place(at, file)
}

/// What a report line says about a part of the session.
#[derive(Default)]
struct Report {
    /// Came across as it was.
    exact: Vec<String>,
    /// Came across, but not quite as it was.
    approx: Vec<String>,
    /// Left out.
    left: Vec<String>,
    /// Audio files not found or not readable: "name: why".
    offline: Vec<String>,
}

impl Report {
    fn approx(&mut self, s: impl Into<String>) {
        push_once(&mut self.approx, s.into());
    }
    fn left(&mut self, s: impl Into<String>) {
        push_once(&mut self.left, s.into());
    }
}

fn push_once(list: &mut Vec<String>, s: String) {
    if list.len() < 2_000 && !list.contains(&s) {
        list.push(s);
    }
}

/// An audio file of the session, as far as it could be read.
#[derive(Clone)]
enum Got {
    /// Decoded at the session rate, with the file it came from.
    Audio { buf: Arc<AudioBuffer>, path: String, format: FileFormat, file_rate: u32 },
    /// Not readable: why, and the path it was looked for at.
    Offline { why: String, path: String },
}

/// Pro Tools time to session samples.
struct Clock {
    /// The Pro Tools session's tempo map (FreeDAWm ticks).
    map: TempoMap,
    sr: SampleRate,
    /// Session rate over the Pro Tools session's rate.
    ratio: f64,
}

impl Clock {
    fn clamp(x: f64) -> Samples {
        if x.is_finite() { (x.round() as i64).clamp(-MAX_POS, MAX_POS) } else { 0 }
    }
    /// A tick-based position (Pro Tools ticks, the `10^12` offset already removed).
    fn ticks(&self, t: u64) -> Samples {
        Self::clamp(self.map.ticks_f_to_samples_f(t as f64 / PT_TICKS_PER_TICK, self.sr))
    }
    /// A sample position or length at the Pro Tools session's rate.
    fn samples(&self, s: i64) -> Samples {
        Self::clamp(s as f64 * self.ratio)
    }
    fn len(&self, n: u64) -> Samples {
        Self::clamp(n as f64 * self.ratio).max(0)
    }
    /// Where a placement sits (its `samples` or its `ticks`).
    fn at(&self, samples: Option<i64>, ticks: Option<u64>) -> Samples {
        match (samples, ticks) {
            (Some(s), _) => self.samples(s),
            (None, Some(t)) => self.ticks(t),
            (None, None) => 0,
        }
    }
    /// A mixer or send lane's time (`ticks` when the lane is tick-based).
    fn lane(&self, t: u64, ticks: bool) -> Samples {
        let t = soundcraft_ptx::primitives::plain_ticks(t);
        if ticks { self.ticks(t) } else { self.samples(i64::try_from(t).unwrap_or(MAX_POS)) }
    }
}

/// The FreeDAWm frame rate for a Pro Tools timecode rate code, and whether it is exactly that.
fn frame_rate(code: u8) -> Option<(FrameRate, bool)> {
    Some(match code {
        1 => (FrameRate::Fps24, true),
        2 => (FrameRate::Fps25, true),
        3 => (FrameRate::Fps29_97, true),
        5 => (FrameRate::Fps29_97Drop, true),
        7 => (FrameRate::Fps30, true),
        8 => (FrameRate::Fps30, false),
        9 => (FrameRate::Fps23_976, true),
        10 => (FrameRate::Fps50, true),
        11 => (FrameRate::Fps59_94, true),
        12 => (FrameRate::Fps59_94Drop, true),
        13 => (FrameRate::Fps60, true),
        14 => (FrameRate::Fps60, false),
        15 => (FrameRate::Fps47_952, true),
        16 => (FrameRate::Fps48, true),
        17 => (FrameRate::Fps100, true),
        18 => (FrameRate::Fps119_88, true),
        19 => (FrameRate::Fps119_88, false),
        20 => (FrameRate::Fps120, true),
        21 => (FrameRate::Fps120, false),
        _ => return None,
    })
}

/// A key signature as the Key ruler names it ("F# minor").
fn key_name(major: bool, sharps: i8) -> String {
    const MAJ_SHARP: [&str; 8] = ["C", "G", "D", "A", "E", "B", "F#", "C#"];
    const MAJ_FLAT: [&str; 8] = ["C", "F", "Bb", "Eb", "Ab", "Db", "Gb", "Cb"];
    const MIN_SHARP: [&str; 8] = ["A", "E", "B", "F#", "C#", "G#", "D#", "A#"];
    const MIN_FLAT: [&str; 8] = ["A", "D", "G", "C", "F", "Bb", "Eb", "Ab"];
    let n = usize::from(sharps.unsigned_abs()).min(7);
    let table = match (major, sharps >= 0) {
        (true, true) => &MAJ_SHARP,
        (true, false) => &MAJ_FLAT,
        (false, true) => &MIN_SHARP,
        (false, false) => &MIN_FLAT,
    };
    format!("{} {}", table.get(n).copied().unwrap_or("C"), if major { "major" } else { "minor" })
}

/// The FreeDAWm palette colour for a Pro Tools colour index. Pro Tools' own colour values are not
/// in the file or published, so this is not a nearest-colour match: the index picks a colour of
/// the palette's grid (hue first, then shade), the same index always the same colour.
fn pt_colour(index: i16) -> Option<(usize, [u8; 3])> {
    let k = usize::try_from(index.checked_sub(25)?).ok()?;
    let hues = soundcraft_model::palette::HUES;
    let shades = soundcraft_model::palette::SHADES;
    let i = soundcraft_model::palette::grid_index((k / hues) % shades, k % hues);
    soundcraft_model::palette::palette_color(i).map(|c| (i, c))
}

/// Tenths of a dB (Pro Tools' fader and send lanes; −1440 is −∞) to dB.
fn tenths_db(v: i16) -> f32 {
    if v <= -1440 { -144.0 } else { (f32::from(v) / 10.0).clamp(-144.0, 12.0) }
}

/// Pro Tools' pan (−100 left … 100 right) to FreeDAWm's (−1 … 1).
fn pan(v: i16) -> f32 {
    (f32::from(v) / 100.0).clamp(-1.0, 1.0)
}

fn fade_shape(f: &FadeObject, track: &str, report: &mut Report) -> FadeShape {
    let curved = f.curve != 0.0 || f.s_curve != [0, 0];
    let shape = match f.slope.as_str() {
        Some("equal_power") => FadeShape::EqualPower,
        Some("equal_gain") => FadeShape::Linear,
        _ => {
            report.approx(format!("\"{track}\": a fade with a slope FreeDAWm has no match for came in equal power"));
            FadeShape::EqualPower
        }
    };
    if curved {
        report.approx(format!("\"{track}\": a fade's custom curve came in as the plain {} shape", shape.label()));
    }
    shape
}

/// Matches a clip's name to an audio file name: itself, without its `.L`/`.R` channel tag, without
/// a `-NN` take suffix, else the longest file name it starts with.
fn match_stem(region: &str, stems: &[String]) -> String {
    let has = |n: &str| stems.iter().any(|s| s == n);
    if has(region) {
        return region.to_string();
    }
    let base = region.strip_suffix(".L").or_else(|| region.strip_suffix(".R")).unwrap_or(region);
    if has(base) {
        return base.to_string();
    }
    if let Some((stem, n)) = base.rsplit_once('-')
        && !n.is_empty()
        && n.bytes().all(|b| b.is_ascii_digit())
        && has(stem)
    {
        return stem.to_string();
    }
    stems.iter().filter(|s| region.starts_with(s.as_str())).max_by_key(|s| s.len()).cloned().unwrap_or_else(|| base.to_string())
}

/// The placements of a playlist: its first channel and (stereo) second, from the decoded lists.
fn playlist_channels(doc: &PtxDocument, name: &str) -> (Vec<AudioPlacement>, Vec<AudioPlacement>) {
    let mut chans: Vec<Vec<AudioPlacement>> = doc
        .audio_channels
        .get(name)
        .map(|chs| chs.iter().map(|v| serde_json::from_value::<Vec<AudioPlacement>>(v.clone()).unwrap_or_default()).collect())
        .unwrap_or_default();
    if chans.first().is_none_or(Vec::is_empty)
        && let Some((_, list)) = doc.audio_playlists.iter().find(|(n, _)| n == name)
    {
        chans = vec![list.clone()];
    }
    // Loops are decoded on the playlist's placements: carry them onto the channel lists.
    if let Some(flat) = doc.audio_placements.get(name) {
        let loops: Vec<AudioPlacement> =
            flat.iter().filter_map(|v| serde_json::from_value::<AudioPlacement>(v.clone()).ok()).filter(|p| p.loop_info.is_some()).collect();
        for ch in &mut chans {
            for e in ch.iter_mut().filter(|e| e.loop_info.is_none() && e.kind.as_str() == Some("clip")) {
                if let Some(l) = loops.iter().find(|l| l.samples == e.samples && l.ticks == e.ticks) {
                    e.loop_info = l.loop_info.clone();
                }
            }
        }
    }
    let mut it = chans.into_iter();
    (it.next().unwrap_or_default(), it.next().unwrap_or_default())
}

/// A clip placed on a playlist, before it becomes a [`Clip`].
struct Placed {
    clip: Clip,
    /// The crossfade starting it, if any: (cut point, samples before the cut, total length).
    xfade_in: Option<(Samples, Samples, Samples)>,
    /// Frames its source has (0: not known).
    frames: Samples,
}

struct Placer<'a> {
    s: &'a mut Session,
    doc: &'a PtxDocument,
    clock: Clock,
    /// The `.ptx`'s folder.
    dir: Option<PathBuf>,
    copy: bool,
    report: Report,
    /// Audio file names (stems) the session lists, in order.
    stems: Vec<String>,
    /// Audio regions (clips' definitions) in file order.
    audio_regions: Vec<&'a Region>,
    midi_regions: Vec<&'a Region>,
    /// Each audio file once read, by stem.
    got: HashMap<String, Got>,
    /// The source playing a stem as it is, or a split-mono pair `(left, right)` combined.
    sources: HashMap<(String, Option<String>), SourceId>,
    /// Pro Tools track index → the FreeDAWm track made from it.
    ids: Vec<Option<TrackId>>,
    clips: usize,
    shift: Samples,
}

impl<'a> Placer<'a> {
    fn new(s: &'a mut Session, doc: &'a PtxDocument, dir: Option<&Path>, copy: bool) -> Placer<'a> {
        let mut report = Report::default();
        let sr = s.sample_rate;
        let pt_rate = doc.format.sample_rate.filter(|r| (8_000..=768_000).contains(r));
        if doc.format.sample_rate.is_some() && pt_rate.is_none() {
            report.approx(format!("the session's sample rate reads as {:?} Hz: taken as the session's own", doc.format.sample_rate));
        }
        let ratio = pt_rate.map_or(1.0, |r| f64::from(sr.hz()) / f64::from(r));
        let mut map = TempoMap::default();
        let mut tempos: Vec<_> = doc.tempo_used.iter().collect();
        tempos.sort_by_key(|t| t.ticks);
        for (i, t) in tempos.iter().enumerate() {
            let bpm = if t.bpm_exact.is_finite() && t.bpm_exact > 0.0 { t.bpm_exact } else { t.bpm };
            let tick = (t.ticks as f64 / PT_TICKS_PER_TICK).round() as i64;
            if t.ticks % 1000 != 0 {
                report.approx("a tempo change between FreeDAWm's 960-a-beat ticks moved to the nearest");
            }
            if (i == 0 && map.set_tempo(0, bpm).is_err()) || map.set_tempo(tick, bpm).is_err() {
                report.left(format!("a tempo of {bpm} BPM (out of range)"));
            }
        }
        let mut meters: Vec<_> = doc.meter_used.iter().collect();
        meters.sort_by_key(|m| m.ticks);
        for m in meters {
            let tick = (m.ticks as f64 / PT_TICKS_PER_TICK).round() as i64;
            if map.set_meter(tick, m.numerator, m.denominator).is_err() {
                report.left(format!("a meter of {}/{} (out of range)", m.numerator, m.denominator));
            } else if !map.meters().iter().any(|x| x.tick == tick) {
                // FreeDAWm changes meter at a bar line: the map moves it to the bar it falls in.
                report.approx(format!("the {}/{} meter change is not on a bar line: it starts at the bar it falls in", m.numerator, m.denominator));
            }
        }
        let mut stems = doc.audio_file_stems.clone();
        if stems.is_empty() {
            stems = doc.umids.keys().cloned().collect();
        }
        Placer {
            clock: Clock { map, sr, ratio },
            dir: dir.map(Path::to_path_buf),
            copy,
            report,
            stems,
            audio_regions: doc.regions.iter().filter(|r| r.list.is_none()).collect(),
            midi_regions: doc.regions.iter().filter(|r| r.list.is_some()).collect(),
            got: HashMap::new(),
            sources: HashMap::new(),
            ids: Vec::new(),
            clips: 0,
            shift: 0,
            s,
            doc,
        }
    }

    fn place(&mut self, at: Option<Samples>, file: &str) -> Result<Value> {
        let doc = self.doc;
        let sr = self.clock.sr;
        let empty = self.s.tracks.iter().all(|t| t.clips().is_empty());
        // Timecode and the session's maps.
        let rate = doc.timeline.timecode_rate_code.and_then(frame_rate);
        // A timecode from the file is checked before it is counted (a crafted one overflows).
        let start_tc = doc
            .timeline
            .session_start
            .as_deref()
            .and_then(|t| Timecode::parse(t).ok())
            .filter(|tc| !tc.negative && tc.hours < 24 && tc.minutes < 60 && tc.seconds < 60 && (0..1000).contains(&tc.frames));
        let pt_start = match (start_tc, rate) {
            (Some(tc), Some((r, _))) => Some(tc.to_samples(r, sr)),
            _ => None,
        };
        self.shift = match at {
            Some(a) => a,
            None if empty => {
                if let Some(start) = pt_start {
                    self.s.timecode_start = start;
                }
                if let Some((r, exact)) = rate {
                    self.s.frame_rate = r;
                    if !exact {
                        self.report.approx(format!(
                            "timecode at {}: FreeDAWm has no such drop-frame rate, the session counts {}",
                            doc.timeline.timecode_rate.as_deref().unwrap_or("?"),
                            r.label()
                        ));
                    }
                }
                0
            }
            None => pt_start.map_or(0, |p| p - self.s.timecode_start),
        };
        if empty {
            self.s.tempo = self.clock.map.clone();
            self.s.key_signatures =
                doc.key_signatures.iter().map(|k| (self.clock.ticks(k.ticks) + self.shift, key_name(k.major, k.sharps))).collect();
            self.s.key_signatures.retain(|(at, _)| *at >= 0);
            self.report.exact.push(format!(
                "tempo map ({}), meters ({}), key signatures ({})",
                doc.tempo_used.len(),
                doc.meter_used.len(),
                doc.key_signatures.len()
            ));
        } else {
            self.report.left("the tempo map, meters and key signatures: the session has clips and keeps its own (positions are kept in time)");
        }
        if let Some(r) = doc.format.sample_rate.filter(|r| *r != sr.hz()) {
            self.report.approx(format!("positions converted from the Pro Tools session's {r} Hz to this session's {} Hz", sr.hz()));
        }

        // Tracks, in order.
        let mut tracks = Vec::new();
        let master = self.s.tracks.iter().find(|t| t.kind == TrackKind::Master).map(|t| t.id);
        for (i, t) in doc.tracks.iter().enumerate() {
            let id = self.track(i, t, master, empty);
            self.ids.push(id);
            if let Some(id) = id
                && Some(id) != master
            {
                tracks.push(id);
            }
        }
        // Folders, once every track is made.
        for (i, t) in doc.tracks.iter().enumerate() {
            let Some(folder) = &t.folder else { continue };
            let parent = doc.tracks.iter().position(|x| &x.name == folder).and_then(|j| self.ids.get(j).copied().flatten());
            let child = self.ids.get(i).copied().flatten();
            if let (Some(parent), Some(child)) = (parent, child)
                && self.s.track(parent).is_some_and(|p| p.kind == TrackKind::Folder)
                && let Some(tr) = self.s.track_mut(child)
            {
                tr.folder = Some(parent);
            }
        }
        self.groups();
        self.markers();

        let markers = doc.markers.len();
        self.finish(file, &tracks, markers)
    }

    /// Makes Pro Tools track `i` (or, for its master, sets the session's master).
    fn track(&mut self, i: usize, t: &PTrack, master: Option<TrackId>, empty: bool) -> Option<TrackId> {
        let format = if t.stereo { ChannelFormat::Stereo } else { ChannelFormat::Mono };
        let kind = if t.track_type == "RoutingFolder" {
            TrackKind::Folder
        } else {
            match t.kind {
                0 => TrackKind::Audio,
                1 => TrackKind::Midi,
                2 => TrackKind::Aux,
                5 => TrackKind::Master,
                7 => TrackKind::Instrument,
                9 => TrackKind::Vca,
                11 => TrackKind::Folder,
                other => {
                    self.report.left(format!("\"{}\": a kind of track FreeDAWm does not know ({other})", t.name));
                    return None;
                }
            }
        };
        let id = if kind == TrackKind::Master {
            match master {
                Some(_) if !empty => {
                    self.report.left(format!("\"{}\" (master fader): the session keeps its own master", t.name));
                    return None;
                }
                Some(m) => m,
                None => self.s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some(&t.name)),
            }
        } else {
            self.s.add_track(kind, format, Some(&t.name))
        };
        if t.kind == 11 {
            self.report.approx(format!("\"{}\": a basic folder came in as a folder track", t.name));
        }
        if kind == TrackKind::Vca {
            self.report.approx(format!("\"{}\": a VCA came in without the tracks it controls (Pro Tools links them through a group)", t.name));
        }
        let colour = t.colour.and_then(pt_colour);
        if let Some(c) = t.colour
            && colour.is_some()
        {
            let name = colour.and_then(|(i, _)| soundcraft_model::palette::palette_name(i)).unwrap_or_default();
            self.report.approx(format!("\"{}\": Pro Tools colour {c} came in as {name} (Pro Tools' colour values are not known here)", t.name));
        }
        if let Some(tr) = self.s.track_mut(id) {
            if let Some((_, c)) = colour {
                tr.color = c;
            }
            if let Some(c) = &t.comment {
                tr.comments = c.clone();
            }
            tr.hidden = t.hidden.unwrap_or(false);
            tr.inactive = t.active == Some(false);
            tr.mixer.solo = t.solo.unwrap_or(false);
            tr.ticks_timebase = t.timebase.as_deref() == Some("Ticks");
        }
        if let Some(m) = &t.elastic_mode {
            self.report.left(format!("\"{}\": Elastic Audio ({m})", t.name));
        }
        if t.user_delay.is_some_and(|d| d != 0) {
            self.report.left(format!("\"{}\": its delay of {} samples", t.name, t.user_delay.unwrap_or(0)));
        }
        if matches!(t.mpe.as_deref(), Some("poly" | "mono")) {
            self.report.left(format!("\"{}\": its MPE mode", t.name));
        }
        self.mixer(i, t, id, kind);
        self.inserts(t, id);
        match self.doc.track_playlists.get(&t.name) {
            Some(TrackPlaylists::Audio { active, alternates, .. }) if kind == TrackKind::Audio => {
                let clips = self.audio_playlist(t, active);
                if let Some(pl) = self.s.track_mut(id).and_then(|tr| tr.playlist_mut()) {
                    pl.clips.extend(clips);
                    pl.sort();
                }
                for alt in alternates {
                    let clips = self.audio_playlist(t, &alt.name);
                    let mut pl = Playlist::new(alt.name.clone());
                    pl.clips = clips;
                    pl.sort();
                    if let Some(tr) = self.s.track_mut(id) {
                        tr.playlists.push(pl);
                    }
                }
            }
            Some(TrackPlaylists::Midi { active_index, alternates, .. }) if kind.is_midi() => {
                let clips = self.midi_playlist(t, *active_index);
                if let Some(pl) = self.s.track_mut(id).and_then(|tr| tr.playlist_mut()) {
                    pl.clips.extend(clips);
                    pl.sort();
                }
                if !alternates.is_empty() {
                    self.report.left(format!("\"{}\": {} alternate MIDI playlists", t.name, alternates.len()));
                }
            }
            _ => {}
        }
        Some(id)
    }

    /// Where a Pro Tools output, input or send named `name` goes: the master's output and the
    /// interface's outputs to the main output, anything else to a bus of that name.
    fn route(&mut self, name: Option<&str>, format: ChannelFormat, what: &str) -> Route {
        let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) else {
            return Route::None;
        };
        let lower = name.to_ascii_lowercase();
        if matches!(lower.as_str(), "none" | "no output" | "no input") {
            return Route::None;
        }
        let master_out = self.doc.tracks.iter().find(|t| t.kind == 5).and_then(|t| t.output.as_deref()).map(str::trim);
        if master_out == Some(name) {
            return Route::Main;
        }
        let is_bus_input = self.doc.tracks.iter().any(|t| t.input.as_deref().map(str::trim) == Some(name));
        if !is_bus_input && (lower.starts_with("out ") || lower.contains("main") || lower.contains("speakers") || lower.contains("headphones")) {
            self.report.approx(format!("{what}: the output \"{name}\" went to the main output"));
            return Route::Main;
        }
        Route::Bus(self.s.add_bus(name, format))
    }

    fn mixer(&mut self, i: usize, t: &PTrack, id: TrackId, kind: TrackKind) {
        let strips: Vec<&Strip> = self.doc.strips.iter().filter(|s| s.track == Some(i)).collect();
        let format = if t.stereo { ChannelFormat::Stereo } else { ChannelFormat::Mono };
        // Aux and routing-folder inputs are buses.
        if matches!(kind, TrackKind::Aux | TrackKind::Folder) && t.input.is_some() {
            let r = self.route(t.input.as_deref(), format, &format!("\"{}\"", t.name));
            if let Some(tr) = self.s.track_mut(id) {
                tr.mixer.input = r;
            }
        }
        let Some(st) = strips.first().copied() else {
            if kind != TrackKind::Midi && kind != TrackKind::Folder && kind != TrackKind::Vca && t.kind != 11 {
                self.report.left(format!("\"{}\": its mixer settings (not found in the file)", t.name));
            }
            return;
        };
        let ticks = st.lane_time_base.as_deref() == Some("ticks");
        let shift = self.shift;
        let out = if kind == TrackKind::Master {
            Route::Main
        } else {
            self.route(st.output.as_deref().or(t.output.as_deref()), ChannelFormat::Stereo, &format!("\"{}\"", t.name))
        };
        let mut sends: Vec<Option<SendSlot>> = Vec::new();
        let mut send_lanes: Vec<(AutoParam, Vec<(Samples, f32)>)> = Vec::new();
        for (k, snd) in st.sends.iter().flatten().enumerate() {
            let target = self.route(snd.output.as_deref(), ChannelFormat::Stereo, &format!("\"{}\" send {}", t.name, k + 1));
            if target == Route::None {
                continue;
            }
            let Ok(slot) = u8::try_from(sends.len()) else { break };
            let mut sl = SendSlot::new(target);
            sl.level_db = snd.volume_db.map_or(0.0, |v| if v.is_finite() { (v as f32).clamp(-144.0, 12.0) } else { -144.0 });
            sl.pan = snd.pan.first().map_or(0.0, |p| pan(p.1));
            sl.mute = snd.mute.first().is_some_and(|m| m.1 > 0);
            sl.pre_fader = snd.post_fader == Some(false);
            sl.follow_main_pan = false;
            let lt = snd.lane_time_base.as_deref() == Some("ticks") || ticks;
            if snd.volume.len() > 1 {
                send_lanes
                    .push((AutoParam::SendLevel(slot), snd.volume.iter().map(|p| (self.clock.lane(p.0, lt) + shift, tenths_db(p.1))).collect()));
            }
            if snd.pan.len() > 1 {
                send_lanes.push((AutoParam::SendPan(slot), snd.pan.iter().map(|p| (self.clock.lane(p.0, lt) + shift, pan(p.1))).collect()));
            }
            if snd.mute.len() > 1 {
                send_lanes.push((
                    AutoParam::SendMute(slot),
                    snd.mute.iter().map(|p| (self.clock.lane(p.0, lt) + shift, f32::from(u8::from(p.1 > 0)))).collect(),
                ));
            }
            sends.push(Some(sl));
        }
        let mut lanes: Vec<(AutoParam, Vec<(Samples, f32)>)> = Vec::new();
        let vol = st.volume_db.map(|v| if v.is_finite() { (v as f32).clamp(-144.0, 12.0) } else { -144.0 });
        if st.volume.len() > 1 {
            lanes.push((AutoParam::Volume, st.volume.iter().map(|p| (self.clock.lane(p.0, ticks) + shift, tenths_db(p.1))).collect()));
        }
        if st.pan.len() > 1 {
            lanes.push((AutoParam::Pan(0), st.pan.iter().map(|p| (self.clock.lane(p.0, ticks) + shift, pan(p.1))).collect()));
        }
        if t.stereo && st.pan2.len() > 1 {
            lanes.push((AutoParam::Pan(1), st.pan2.iter().map(|p| (self.clock.lane(p.0, ticks) + shift, pan(p.1))).collect()));
        }
        if st.mute.len() > 1 {
            lanes.push((AutoParam::Mute, st.mute.iter().map(|p| (self.clock.lane(p.0, ticks) + shift, f32::from(u8::from(p.1 > 0)))).collect()));
        }
        if st.lfe_db.is_some_and(|v| v.is_finite() && v > -140.0) {
            self.report.left(format!("\"{}\": its LFE level", t.name));
        }
        let mut dropped = 0;
        let n_sends = sends.len();
        let Some(tr) = self.s.track_mut(id) else { return };
        if let Some(v) = vol {
            tr.mixer.volume_db = v;
        }
        match tr.mixer.pan.len() {
            1 => {
                if let Some(p) = st.pan.first() {
                    tr.mixer.pan = vec![pan(p.1)];
                }
            }
            2 => {
                let l = st.pan.first().map_or(-1.0, |p| pan(p.1));
                let r = st.pan2.first().map_or(1.0, |p| pan(p.1));
                tr.mixer.pan = vec![l, r];
            }
            _ => {}
        }
        tr.mixer.mute = st.mute.first().is_some_and(|m| m.1 > 0);
        if out != Route::None {
            tr.mixer.output = out;
        }
        for (k, sl) in sends.into_iter().enumerate() {
            match tr.mixer.sends.get_mut(k) {
                Some(slot) => *slot = sl,
                None => dropped += 1,
            }
        }
        let mut shown = Vec::new();
        for (param, pts) in lanes.into_iter().chain(send_lanes) {
            if let AutoParam::SendLevel(k) | AutoParam::SendPan(k) | AutoParam::SendMute(k) = param
                && usize::from(k) >= tr.mixer.sends.len()
            {
                continue;
            }
            let lane = tr.lane_mut(&param);
            lane.points.clear();
            for (at, v) in pts {
                lane.set_point(at.max(0), v);
            }
            shown.push(param.id());
        }
        for l in shown {
            if !tr.auto_lanes.contains(&l) {
                tr.auto_lanes.push(l);
            }
        }
        if dropped > 0 {
            self.report.left(format!("\"{}\": {dropped} of its {n_sends} sends (FreeDAWm has fewer send slots)", t.name));
        }
    }

    /// The stock plug-ins with a clear FreeDAWm equivalent; every other insert is listed.
    fn inserts(&mut self, t: &PTrack, id: TrackId) {
        let Some(list) = &t.inserts else { return };
        for ins in list {
            let name = ins.plugin.clone().unwrap_or_else(|| if ins.name.is_empty() { "an unknown plug-in".into() } else { ins.name.clone() });
            let slot = ins.slot.as_str().map(str::to_string).unwrap_or_else(|| ins.slot.to_string());
            let mapped = match name.as_str() {
                "EQ3 7-Band" => Some(eq7_to_cleanq(ins)),
                "EQ3 1-Band" => Some(eq1_to_cleanq(ins)),
                "Dyn3 Compressor/Limiter" => Some(dyn3_to_compressor(ins)),
                "Trim" => Some(trim_to_trim(ins)),
                _ => None,
            };
            let Some((mut module, note)) = mapped else {
                self.report
                    .left(format!("\"{}\": {name} (insert {slot}): not carried over, FreeDAWm has no plug-in that is clearly the same", t.name));
                continue;
            };
            module.bypass = ins.bypass.unwrap_or(false);
            module.active = ins.active.unwrap_or(true);
            let ours = soundcraft_dsp::plugin_info(&module.plugin).map_or_else(|| module.plugin.clone(), |i| i.name.to_string());
            let added = self.s.track_mut(id).map(|tr| tr.mixer.add_module(module, None).is_ok()).unwrap_or(false);
            if !added {
                self.report.left(format!("\"{}\": {name} (insert {slot}): the track's chain is full", t.name));
                continue;
            }
            match note {
                Some(n) => self.report.approx(format!("\"{}\": {name} (insert {slot}) came in as {ours}: {n}", t.name)),
                None => self.report.exact.push(format!("\"{}\": {name} (insert {slot}) as {ours}", t.name)),
            }
            if ins.automation.iter().any(|l| l.len() > 1) {
                self.report.left(format!("\"{}\": the automation of {name} (insert {slot})", t.name));
            }
        }
    }

    // ---- audio ------------------------------------------------------------------------------

    /// Audio file `stem`, read on first use from the session's `Audio Files` or beside it.
    fn got(&mut self, stem: &str) -> Got {
        if let Some(g) = self.got.get(stem) {
            return g.clone();
        }
        let sr = self.clock.sr.hz();
        let ext = self.doc.audio_file_exts.get(stem).cloned();
        let exts: Vec<String> = match &ext {
            Some(x) => vec![x.clone()],
            None => ["wav", "WAV", "aif", "aiff", "AIF", "AIFF"].iter().map(|x| x.to_string()).collect(),
        };
        let mut why = "not found".to_string();
        let mut last = format!("Audio Files/{stem}.{}", exts.first().map_or("wav", String::as_str));
        let g = 'found: {
            if ext.as_deref().is_some_and(|x| x.eq_ignore_ascii_case("mxf") || x.eq_ignore_ascii_case("sd2")) {
                why = format!("{} audio is not read", ext.as_deref().unwrap_or("").to_ascii_uppercase());
                break 'found Got::Offline { why, path: last };
            }
            let Some(dir) = self.dir.clone() else { break 'found Got::Offline { why, path: last } };
            for x in &exts {
                let file = format!("{stem}.{x}");
                for cand in [dir.join("Audio Files").join(&file), dir.join(&file)] {
                    if !crate::io::file_found(&cand) {
                        continue;
                    }
                    let shown = cand.to_string_lossy().into_owned();
                    last = shown.clone();
                    let read = crate::io::read_file_max(&cand, MAX_MEDIA_BYTES).map_err(|e| e.to_string()).and_then(|b| {
                        let (info, buf) = soundcraft_audio_io::decode(&b, Some(&x.to_ascii_lowercase())).map_err(|e| e.to_string())?;
                        if buf.frames() == 0 {
                            return Err("no audio".to_string());
                        }
                        let buf = crate::io::at_rate(buf, sr, crate::io::MAX_IMPORT_SAMPLES).ok_or("too large to import")?;
                        Ok((info, buf))
                    });
                    match read {
                        Ok((info, buf)) => {
                            break 'found Got::Audio { buf: Arc::new(buf), path: shown, format: info.format, file_rate: info.sample_rate };
                        }
                        Err(w) => why = w,
                    }
                }
            }
            Got::Offline { why, path: last }
        };
        self.got.insert(stem.to_string(), g.clone());
        g
    }

    /// The source playing `stem` (a stereo track's split-mono pair: `left` and `right` combined),
    /// with the frames it has (0 when offline).
    fn source(&mut self, left: &str, right: Option<&str>, channels: u16, frames_hint: u64) -> (SourceId, Samples) {
        let key = (left.to_string(), right.map(str::to_string));
        if let Some(&id) = self.sources.get(&key) {
            let frames = self.s.source(id).map_or(0, |x| i64::try_from(x.frames).unwrap_or(0));
            return (id, frames);
        }
        let sr = self.clock.sr.hz();
        let id = match right {
            None => match self.got(left) {
                Got::Audio { buf, path, format, file_rate } => {
                    let audio = Arc::new(soundcraft_model::SourceAudio::new((*buf).clone()));
                    let id = crate::io::insert_source(self.s, left, audio, Some(&path), format);
                    // Referred to where it is, as File › Import › Audio, when the session does not
                    // copy imported audio and the file is at its rate.
                    if !self.copy
                        && Path::new(&path).is_absolute()
                        && file_rate == sr
                        && let Some(sx) = self.s.sources.last_mut()
                    {
                        sx.unsaved = false;
                    }
                    id
                }
                Got::Offline { why, path } => {
                    push_once(&mut self.report.offline, format!("{left}: {why}"));
                    self.offline_source(left, &path, channels, frames_hint)
                }
            },
            Some(right) => {
                let (l, r) = (self.got(left), self.got(right));
                let base = left.strip_suffix(".L").unwrap_or(left).to_string();
                match (&l, &r) {
                    (Got::Audio { buf: a, .. }, Got::Audio { buf: b, .. }) => {
                        let frames = a.frames().max(b.frames());
                        let chan = |x: &AudioBuffer| {
                            let mut c = x.channels.first().cloned().unwrap_or_default();
                            c.resize(frames, 0.0);
                            c
                        };
                        let buf = AudioBuffer { sample_rate: sr, channels: vec![chan(a), chan(b)] };
                        self.report.approx(format!("{base}: the split-mono pair {left} and {right} became one stereo file in the session"));
                        crate::io::add_source(self.s, &base, buf, None, FileFormat::Wav)
                    }
                    _ => {
                        for (n, g) in [(left, &l), (right, &r)] {
                            if let Got::Offline { why, .. } = g {
                                push_once(&mut self.report.offline, format!("{n}: {why}"));
                            }
                        }
                        let path = match &l {
                            Got::Offline { path, .. } => path.clone(),
                            Got::Audio { path, .. } => path.clone(),
                        };
                        self.offline_source(&base, &path, 2, frames_hint)
                    }
                }
            }
        };
        self.sources.insert(key, id);
        let frames = self.s.source(id).map_or(0, |x| i64::try_from(x.frames).unwrap_or(0));
        (id, frames)
    }

    /// A source with no audio, as missing audio is when a session opens without it.
    fn offline_source(&mut self, name: &str, path: &str, channels: u16, frames: u64) -> SourceId {
        let id = SourceId(self.s.alloc());
        let sr = self.clock.sr.hz();
        self.s.sources.push(Source {
            id,
            name: name.to_string(),
            path: path.to_string(),
            channels: channels.max(1),
            frames,
            sample_rate: sr,
            format: FileFormat::Wav,
            time_reference: 0,
            unsaved: false,
        });
        id
    }

    /// The clips of the audio playlist `name` of track `t`.
    fn audio_playlist(&mut self, t: &PTrack, name: &str) -> Vec<Clip> {
        let (ch0, ch1) = playlist_channels(self.doc, name);
        // Clip groups are opened out into their clips (the clips keep a shared group).
        let mut expanded: Vec<(AudioPlacement, Option<AudioPlacement>, Option<u64>)> = Vec::new();
        for p in &ch0 {
            let partner = ch1.iter().find(|x| x.kind == p.kind && x.samples == p.samples && x.ticks == p.ticks).cloned();
            if p.kind.as_str() == Some("group") {
                let Some(members) = self
                    .doc
                    .clip_group_regions
                    .get(usize::from(p.index))
                    .and_then(|g| g.member_playlist_index)
                    .and_then(|m| usize::try_from(m).ok())
                    .and_then(|m| self.doc.clip_group_playlists.get(m))
                else {
                    self.report.left(format!("\"{}\": a clip group whose clips are not in the file", t.name));
                    continue;
                };
                let gid = self.s.alloc();
                let gname = self.doc.clip_group_regions.get(usize::from(p.index)).map(|g| g.name.clone()).unwrap_or_default();
                self.report
                    .approx(format!("\"{}\": the clip group \"{gname}\" came in as its clips, grouped (the group's own name is not kept)", t.name));
                let base = self.clock.at(p.samples, p.ticks);
                for m in members {
                    let mut m = m.clone();
                    let rel = self.clock.at(m.samples, m.ticks);
                    // Members are relative to the group: placed in session samples at PT's rate.
                    m.samples = Some(((base + rel) as f64 / self.clock.ratio).round() as i64);
                    m.ticks = None;
                    m.clip_start_samples = None;
                    expanded.push((m, None, Some(gid)));
                }
            } else {
                expanded.push((p.clone(), partner, None));
            }
        }
        let fades: Vec<(Samples, &FadeObject)> = expanded
            .iter()
            .filter(|(p, _, _)| p.kind.as_str() == Some("fade"))
            .filter_map(|(p, _, _)| Some((self.clock.at(p.samples, p.ticks), self.doc.fades.get(usize::from(p.index))?)))
            .collect();
        let mut placed: Vec<Placed> = Vec::new();
        for (p, partner, group) in &expanded {
            if p.kind.as_str() != Some("clip") {
                continue;
            }
            if self.clips >= MAX_CLIPS {
                self.report.left(format!("clips past the first {MAX_CLIPS}"));
                break;
            }
            let Some(reg) = self.audio_regions.get(usize::from(p.index)).copied() else {
                self.report.left(format!("\"{}\": a clip whose definition is not in the file", t.name));
                continue;
            };
            let stem = reg.file_index.and_then(|fi| self.doc.audio_file_stems.get(fi)).cloned().unwrap_or_else(|| match_stem(&reg.name, &self.stems));
            let right = partner.as_ref().filter(|_| t.stereo).and_then(|q| self.audio_regions.get(usize::from(q.index))).map(|r1| {
                r1.file_index.and_then(|fi| self.doc.audio_file_stems.get(fi)).cloned().unwrap_or_else(|| match_stem(&r1.name, &self.stems))
            });
            let right = right.filter(|r| *r != stem);
            let channels = if t.stereo { 2 } else { 1 };
            let (src, frames) = self.source(&stem, right.as_deref(), channels, reg.start.saturating_add(reg.length));
            let sync = self.clock.len(reg.sync_offset.unwrap_or(0));
            let at = match p.clip_start_samples {
                Some(cs) => self.clock.samples(cs),
                None => self.clock.at(p.samples, p.ticks) - sync,
            };
            let start = at + self.shift;
            let len = self.clock.len(reg.length).max(1);
            let offset = self.clock.len(reg.start);
            let mut clip = Clip::audio(self.s.new_clip_id(), reg.name.clone(), src, offset, start, len);
            clip.sync_point = sync.clamp(0, len);
            clip.muted = p.muted;
            clip.group = *group;
            // Clip gain: its envelope when it has more than one point, else the static gain.
            let lane = reg.clip_gain_index.flatten().and_then(|k| self.doc.clip_gain_lanes.get(k)).filter(|l| l.points.len() > 1);
            match lane {
                Some(l) => {
                    clip.gain_env =
                        l.points.iter().map(|(o, db)| (self.clock.len(u64::from(*o)).min(len), (*db as f32).clamp(-144.0, 24.0))).collect();
                }
                None => clip.gain_db = reg.clip_gain.map_or(0.0, |g| if g.is_finite() { (g as f32).clamp(-144.0, 24.0) } else { 0.0 }),
            }
            let end = start + len;
            let mut xfade_in = None;
            for (fs, f) in &fades {
                let fs = fs + self.shift;
                let total = self.clock.len(f.length2.unwrap_or(f.length));
                let pre = if f.length2.is_some() { self.clock.len(f.length) } else { 0 };
                match f.fade_type.as_str() {
                    Some("fade_in") if fs == start => {
                        clip.fade_in =
                            Fade { len: self.clock.len(f.length).min(len), shape: fade_shape(f, &t.name, &mut self.report), curvature: 0.0 };
                    }
                    Some("fade_out") if fs == end || fs + self.clock.len(f.length) == end => {
                        clip.fade_out =
                            Fade { len: self.clock.len(f.length).min(len), shape: fade_shape(f, &t.name, &mut self.report), curvature: 0.0 };
                    }
                    Some("crossfade") if fs == start || fs + pre == start => {
                        let cut = start;
                        clip.fade_in = Fade { len: total.min(len), shape: fade_shape(f, &t.name, &mut self.report), curvature: 0.0 };
                        xfade_in = Some((cut, pre, total));
                    }
                    Some("crossfade") if fs == end || fs + pre == end => {
                        clip.fade_out = Fade { len: total.min(len), shape: fade_shape(f, &t.name, &mut self.report), curvature: 0.0 };
                    }
                    _ => {}
                }
            }
            self.clips += 1;
            match &p.loop_info {
                Some(lp) => self.looped(t, clip, lp, frames, &mut placed),
                None => placed.push(Placed { clip, xfade_in, frames }),
            }
        }
        self.crossfades(t, &mut placed);
        placed.into_iter().map(|p| p.clip).collect()
    }

    /// A Pro Tools clip loop as Clip › Loop's copies (sharing a loop id).
    fn looped(&mut self, t: &PTrack, clip: Clip, lp: &ClipLoop, frames: Samples, placed: &mut Vec<Placed>) {
        let count = u64::from(lp.count.max(1)).min(MAX_LOOP_COPIES);
        let iter = if let Some(n) = lp.iter_samples.filter(|&n| n > 0) {
            self.clock.len(n)
        } else if let Some(n) = lp.iter_ticks.filter(|&n| n > 0) {
            let t0 = self.clock.map.samples_to_ticks_f(clip.start - self.shift, self.clock.sr) * PT_TICKS_PER_TICK;
            let t0 = if t0.is_finite() && t0 > 0.0 { t0 as u64 } else { 0 };
            self.clock.ticks(t0.saturating_add(n)) - (clip.start - self.shift)
        } else {
            clip.length
        }
        .max(1);
        if u64::from(lp.count) > MAX_LOOP_COPIES {
            self.report.left(format!("\"{}\": loop copies of \"{}\" past {MAX_LOOP_COPIES}", t.name, clip.name));
        }
        if iter == clip.length {
            self.report.exact.push(format!("\"{}\": the loop of \"{}\" as {count} looped copies", t.name, clip.name));
        } else {
            self.report.approx(format!(
                "\"{}\": the loop of \"{}\" repeats every {iter} samples, the clip is {}: {count} copies each that far apart",
                t.name, clip.name, clip.length
            ));
        }
        let loop_id = Some(self.s.alloc());
        for k in 0..count {
            let mut c = clip.clone();
            c.id = self.s.new_clip_id();
            c.start = (clip.start + iter.saturating_mul(k as i64)).min(MAX_POS);
            c.length = clip.length.min(iter);
            c.loop_id = loop_id;
            if k > 0 {
                c.fade_in = Fade::default();
            }
            if k + 1 < count {
                c.fade_out = Fade::default();
            }
            self.clips += 1;
            placed.push(Placed { clip: c, xfade_in: None, frames });
        }
    }

    /// Crossfades: the two clips are lengthened into each other where their audio reaches
    /// (Pro Tools plays the handles), else the fades stay inside the clips.
    fn crossfades(&mut self, t: &PTrack, placed: &mut [Placed]) {
        placed.sort_by_key(|p| p.clip.start);
        for k in 1..placed.len() {
            let Some((cut, pre, total)) = placed.get(k).and_then(|p| p.xfade_in) else { continue };
            let (left, right) = placed.split_at_mut(k);
            let (Some(a), Some(b)) = (left.last_mut(), right.first_mut()) else { continue };
            let xs = cut - pre;
            let xe = xs + total;
            let ext_a = (xe - a.clip.end()).max(0);
            let ext_b = (b.clip.start - xs).max(0);
            let a_off = match a.clip.content {
                soundcraft_model::ClipContent::Audio { offset, .. } => offset,
                _ => 0,
            };
            let a_room = a.frames > 0 && a_off + a.clip.length + ext_a <= a.frames;
            let b_room = matches!(b.clip.content, soundcraft_model::ClipContent::Audio { offset, .. } if offset >= ext_b) && b.frames > 0;
            if a.clip.end() == cut && a_room && b_room {
                a.clip.length += ext_a;
                b.clip.start -= ext_b;
                b.clip.length += ext_b;
                if let soundcraft_model::ClipContent::Audio { offset, .. } = &mut b.clip.content {
                    *offset -= ext_b;
                }
                a.clip.fade_out.len = total.min(a.clip.length);
                b.clip.fade_in.len = total.min(b.clip.length);
                self.report.exact.push(format!("\"{}\": the crossfade at sample {cut}", t.name));
            } else {
                a.clip.fade_out.len = a.clip.fade_out.len.max(total.min(a.clip.length));
                self.report.approx(format!(
                    "\"{}\": the crossfade at sample {cut} became a fade out and a fade in (the audio past the cut is not there to overlap)",
                    t.name
                ));
            }
        }
    }

    // ---- MIDI -------------------------------------------------------------------------------

    fn midi_playlist(&mut self, t: &PTrack, active: usize) -> Vec<Clip> {
        let Some(places) = self.doc.placements.get(active) else { return Vec::new() };
        let mut out = Vec::new();
        for mp in places {
            if self.clips >= MAX_CLIPS {
                self.report.left(format!("clips past the first {MAX_CLIPS}"));
                break;
            }
            let Some(reg) = self.midi_regions.get(usize::from(mp.region)).copied() else {
                self.report.left(format!("\"{}\": a MIDI clip whose definition is not in the file", t.name));
                continue;
            };
            let chunk = reg.list.and_then(|li| usize::try_from(li).ok()).and_then(|i| self.doc.midi_lists.get(i));
            let seq = chunk.map(|c| self.sequence(t, c, reg)).unwrap_or_default();
            let (start, len) = self.midi_span(mp, reg);
            self.clips += 1;
            let mut clip = Clip::midi(self.s.new_clip_id(), reg.name.clone(), start + self.shift, len, seq);
            clip.muted = mp.muted;
            match &mp.loop_info {
                Some(lp) => {
                    let mut placed = Vec::new();
                    self.looped(t, clip, lp, 0, &mut placed);
                    out.extend(placed.into_iter().map(|p| p.clip));
                }
                None => out.push(clip),
            }
        }
        out
    }

    /// Where a MIDI clip starts and how long it is, in session samples.
    fn midi_span(&self, mp: &MidiPlacement, reg: &Region) -> (Samples, Samples) {
        match (mp.samples, mp.ticks) {
            (Some(s), _) => {
                let start = self.clock.samples(s);
                let t0 = self.clock.map.samples_to_ticks_f(start, self.clock.sr) * PT_TICKS_PER_TICK;
                let t0 = if t0.is_finite() && t0 > 0.0 { t0 as u64 } else { 0 };
                (start, (self.clock.ticks(t0.saturating_add(reg.length)) - start).max(1))
            }
            (None, t) => {
                let t = t.unwrap_or(0);
                let start = self.clock.ticks(t);
                (start, (self.clock.ticks(t.saturating_add(reg.length)) - start).max(1))
            }
        }
    }

    /// A MIDI chunk's notes and controllers inside region `reg`, in FreeDAWm ticks from its start.
    fn sequence(&mut self, t: &PTrack, c: &MidiChunk, reg: &Region) -> Sequence {
        let from = reg.start;
        let to = reg.start.saturating_add(reg.length);
        let tick = |x: u64| ((x.saturating_sub(from)) as f64 / PT_TICKS_PER_TICK).round() as i64;
        let mut rounded = false;
        let mut seq = Sequence::default();
        for &[start, key, len, vel] in &c.notes {
            if start < from || start >= to || key > 127 {
                continue;
            }
            rounded |= start % 1000 != 0 || len % 1000 != 0;
            seq.notes.push(Note {
                pitch: key as u8,
                velocity: vel.clamp(1, 127) as u8,
                release_velocity: 64,
                channel: 0,
                start: tick(start),
                length: ((len as f64 / PT_TICKS_PER_TICK).round() as i64).clamp(1, MAX_POS),
                muted: false,
                probability: 100,
            });
        }
        let ctrl = |seq: &mut Sequence, at: u64, kind: CtrlKind, value: i32, channel: u8| {
            if at >= from && at < to {
                seq.ctrls.push(CtrlEvent { tick: tick(at), channel, kind, value, shape: CtrlShape::default() });
            }
        };
        for k in &c.controllers {
            let kind = match k.kind.as_str() {
                Some("pitch_bend") => CtrlKind::PitchBend,
                Some("controller") => CtrlKind::Cc(k.number.min(127)),
                Some("channel_pressure") => CtrlKind::ChannelPressure,
                Some("poly_pressure") => CtrlKind::PolyPressure(k.number.min(127)),
                Some("program") => CtrlKind::Program,
                _ => {
                    self.report.left(format!("\"{}\": a kind of MIDI controller FreeDAWm does not know", t.name));
                    continue;
                }
            };
            if let (CtrlKind::Program, Some(pbs)) = (kind, &k.program_banks) {
                for pb in pbs {
                    let banked = pb.bank_msb != 0 || pb.bank_lsb != 0;
                    if banked {
                        ctrl(&mut seq, pb.ticks, CtrlKind::Cc(0), pb.bank_msb.min(127) as i32, 0);
                        ctrl(&mut seq, pb.ticks, CtrlKind::Cc(32), pb.bank_lsb.min(127) as i32, 0);
                    }
                    ctrl(&mut seq, pb.ticks, CtrlKind::Program, pb.program.min(127) as i32, 0);
                }
                continue;
            }
            let (lo, hi) = if kind == CtrlKind::PitchBend { (-8192, 8191) } else { (0, 127) };
            for &[at, v] in &k.points {
                let at = u64::try_from(at.max(0)).unwrap_or(0);
                ctrl(&mut seq, at, kind, v.clamp(lo, hi) as i32, 0);
            }
        }
        for (at, hex) in &c.sysex {
            let bytes: Vec<u8> = (0..hex.len() / 2).filter_map(|i| hex.get(2 * i..2 * i + 2).and_then(|b| u8::from_str_radix(b, 16).ok())).collect();
            if *at >= from
                && *at < to
                && let Some(x) = SysEx::new(tick(*at), &bytes)
            {
                seq.sysex.push(x);
            }
        }
        if !c.mpe_curves.is_empty() {
            self.report.left(format!("\"{}\": per-note MPE expression", t.name));
        }
        if rounded {
            self.report.approx(format!("\"{}\": notes between FreeDAWm's 960-a-beat ticks moved to the nearest", t.name));
        }
        seq.sort();
        seq
    }

    // ---- groups and markers -----------------------------------------------------------------

    fn groups(&mut self) {
        let mut letters: Vec<char> = self.s.groups.iter().map(|g| g.letter).collect();
        for gd in &self.doc.group_details {
            if gd.name == "<ALL>" || gd.id == u32::MAX {
                continue;
            }
            let members: Vec<TrackId> = gd.member_indices.iter().filter_map(|&k| self.ids.get(usize::from(k)).copied().flatten()).collect();
            let Some(letter) = ('a'..='z').find(|c| !letters.contains(c)) else {
                self.report.left(format!("the group \"{}\" (no group letter left)", gd.name));
                continue;
            };
            letters.push(letter);
            let id = GroupId(self.s.alloc());
            let color = soundcraft_model::palette::palette_color_wrapping(u64::from(letter as u32 - 'a' as u32));
            self.s.groups.push(Group {
                id,
                name: gd.name.clone(),
                letter,
                members,
                edit: gd.group_type & 1 != 0,
                mix: gd.group_type & 2 != 0 || gd.group_type == 0,
                active: gd.enabled,
                color,
                attributes: Vec::new(),
            });
            self.report.exact.push(format!("the group \"{}\"", gd.name));
            if gd.attributes.is_some_and(|a| a > 1) {
                self.report.approx(format!("the group \"{}\": which controls it links (Pro Tools' attributes) are not read", gd.name));
            }
        }
    }

    fn markers(&mut self) {
        for m in &self.doc.markers {
            let to = |c: &Clock, pos: u64| if m.unit == "samples" { c.samples(i64::try_from(pos).unwrap_or(MAX_POS)) } else { c.ticks(pos) };
            let start = to(&self.clock, m.samples.unwrap_or(m.ticks)) + self.shift;
            let end =
                if m.end > m.ticks || (m.unit == "samples" && m.end > m.samples.unwrap_or(0)) { to(&self.clock, m.end) + self.shift } else { start };
            if start < 0 {
                self.report.left(format!("the marker \"{}\" (before the session start)", m.name));
                continue;
            }
            let kind = if m.kind.as_str() == Some("selection") { MarkerKind::Selection } else { MarkerKind::Marker };
            let id = self.s.add_marker(&m.name, kind, start, if kind == MarkerKind::Selection { end } else { start });
            if let Some(loc) = self.s.markers.iter_mut().find(|l| l.id == id)
                && let Some(c) = m.comment.as_ref().filter(|c| !c.is_empty())
            {
                loc.comments = c.clone();
            }
        }
    }

    // ---- the report -------------------------------------------------------------------------

    fn finish(&mut self, file: &str, tracks: &[TrackId], markers: usize) -> Result<Value> {
        let doc = self.doc;
        for w in &doc.warnings {
            self.report.left(format!("the reader: {w}"));
        }
        let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut summary = format!("Pro Tools {file}: {}, {}", plural(tracks.len(), "track", "tracks"), plural(self.clips, "clip", "clips"));
        if markers > 0 {
            summary.push_str(&format!(", {}", plural(markers, "marker", "markers")));
        }
        if !self.report.offline.is_empty() {
            summary.push_str(&format!("; {} offline", plural(self.report.offline.len(), "file", "files")));
        }
        if !self.report.approx.is_empty() {
            summary.push_str(&format!("; {} approximated", plural(self.report.approx.len(), "thing", "things")));
        }
        if !self.report.left.is_empty() {
            summary.push_str(&format!("; {} not carried (see Notes)", plural(self.report.left.len(), "thing", "things")));
        }
        let mut lines = vec![summary.clone()];
        let mut about = Vec::new();
        if let Some(v) = &doc.version {
            about.push(format!("saved by Pro Tools {v}"));
        }
        if let Some(r) = doc.format.sample_rate {
            about.push(format!("{r} Hz"));
        }
        if let Some(b) = doc.format.bit_depth {
            about.push(format!("{b}-bit {}", doc.format.file_format.as_deref().unwrap_or("")).trim_end().to_string());
        }
        if let (Some(r), Some(st)) = (&doc.timeline.timecode_rate, &doc.timeline.session_start) {
            about.push(format!("starts at {st} ({r} fps)"));
        }
        if !about.is_empty() {
            lines.push(format!("The session: {}.", about.join(", ")));
        }
        for x in &self.report.exact {
            lines.push(format!("Exact: {x}"));
        }
        for x in &self.report.approx {
            lines.push(format!("Approximated: {x}"));
        }
        for x in &self.report.offline {
            lines.push(format!("Offline: {x}"));
        }
        for x in &self.report.left {
            lines.push(format!("Not carried: {x}"));
        }
        let text = lines.join("\n");
        let id = self.s.alloc();
        let mut note: String = text.chars().take(soundcraft_model::MAX_NOTE_CHARS.saturating_sub(1)).collect();
        if note.len() < text.len() {
            note.push('…');
        }
        self.s.notes.push(soundcraft_model::Note { id, text: note, done: false, tags: vec!["ptx".into()], at: None, created: String::new() });
        soundcraft_model::sanitize_notes(&mut self.s.notes);
        self.s.sanitize();
        Ok(json!({
            "name": Path::new(file).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default(),
            "version": doc.version,
            "tracks": tracks,
            "clips": self.clips,
            "markers": markers,
            "offline": self.report.offline,
            "approximated": self.report.approx,
            "skipped": self.report.left,
            "exact": self.report.exact,
            "summary": summary,
            "report_text": text,
        }))
    }
}

// ---- stock plug-ins ---------------------------------------------------------------------------

fn num(ins: &PInsert, key: &str) -> Option<f64> {
    ins.params.get(key).and_then(Value::as_f64).filter(|v| v.is_finite())
}

/// One CleanQ band: on, type, frequency (Hz), gain (dB), Q, slope index.
fn cleanq_band(m: &mut Insert, band: usize, on: bool, kind: i32, freq: f64, gain: f64, q: f64, slope: Option<f64>) {
    use stock::cleanq::clean_q_dsp as cq;
    let b = format!("b{band:02}_");
    m.params.insert(format!("{b}on"), if on { 2.0 } else { 0.0 });
    m.params.insert(format!("{b}type"), kind as f32);
    m.params.insert(format!("{b}freq"), cq::freq_to_knob(freq.clamp(10.0, 20_000.0)).clamp(0.0, 1.0));
    m.params.insert(format!("{b}gain"), cq::gain_to_knob(gain.clamp(-30.0, 30.0)).clamp(0.0, 1.0));
    m.params.insert(format!("{b}q"), cq::q_to_knob(q.clamp(0.025, 40.0)).clamp(0.0, 1.0));
    if let Some(s) = slope {
        m.params.insert(format!("{b}slope"), s.clamp(0.0, 7.0).round() as f32);
    }
}

/// EQ3 7-Band as CleanQ: HPF, LF, LMF, MF, HMF, HF, LPF as bands 1–7 in that order, the input
/// and output gain summed into CleanQ's output (the same for a linear EQ).
fn eq7_to_cleanq(ins: &PInsert) -> (Insert, Option<String>) {
    use stock::cleanq::clean_q_dsp as cq;
    let mut m = Insert::new("fs_cleanq");
    let on = |n: u8| ins.params.get(&format!("l_in {n}")).and_then(Value::as_i64).is_some_and(|v| v != 0);
    let f = |key: &str, d: f64| num(ins, key).unwrap_or(d);
    cleanq_band(&mut m, 1, on(1), cq::TYPE_LOW_CUT, f("d_frq1", 20.0), 0.0, 0.707, num(ins, "l_hpfs").or(Some(1.0)));
    cleanq_band(&mut m, 2, on(2), cq::TYPE_LOW_SHELF, f("d_frq2", 100.0), f("d_gai2", 0.0), f("d_bnd2", 1.0), None);
    cleanq_band(&mut m, 3, on(3), cq::TYPE_BELL, f("d_frq3", 200.0), f("d_gai3", 0.0), f("d_bnd3", 1.0), None);
    cleanq_band(&mut m, 4, on(4), cq::TYPE_BELL, f("d_frq4", 1000.0), f("d_gai4", 0.0), f("d_bnd4", 1.0), None);
    cleanq_band(&mut m, 5, on(5), cq::TYPE_BELL, f("d_frq5", 2000.0), f("d_gai5", 0.0), f("d_bnd5", 1.0), None);
    cleanq_band(&mut m, 6, on(6), cq::TYPE_HIGH_SHELF, f("d_frq6", 6000.0), f("d_gai6", 0.0), f("d_bnd6", 1.0), None);
    cleanq_band(&mut m, 7, on(7), cq::TYPE_HIGH_CUT, f("d_frq7", 20_000.0), 0.0, 0.707, num(ins, "l_lpfs").or(Some(1.0)));
    let out = f("d_inpt", 0.0) + f("d_outp", 0.0);
    m.params.insert("outGain".into(), cq::out_db_to_knob(out.clamp(-24.0, 24.0)).clamp(0.0, 1.0));
    let note = "the low and high bands taken as shelves and the filters' slopes as 6/12/18/24 dB an octave (Pro Tools' peak/shelf switches are not read); \
                CleanQ's filters and shelves are its own designs";
    (m, Some(note.into()))
}

/// EQ3 1-Band as one CleanQ band.
fn eq1_to_cleanq(ins: &PInsert) -> (Insert, Option<String>) {
    use stock::cleanq::clean_q_dsp as cq;
    let mut m = Insert::new("fs_cleanq");
    let kind = match ins.params.get("l_type").and_then(Value::as_i64) {
        Some(0) => cq::TYPE_LOW_CUT,
        Some(1) => cq::TYPE_HIGH_CUT,
        Some(2) => cq::TYPE_NOTCH,
        Some(4) => cq::TYPE_LOW_SHELF,
        Some(5) => cq::TYPE_HIGH_SHELF,
        _ => cq::TYPE_BELL,
    };
    cleanq_band(
        &mut m,
        1,
        true,
        kind,
        num(ins, "d_ctrf").unwrap_or(1000.0),
        num(ins, "d_gain").unwrap_or(0.0),
        num(ins, "d_bndw").unwrap_or(1.0),
        None,
    );
    m.params.insert("outGain".into(), cq::out_db_to_knob(num(ins, "d_trim").unwrap_or(0.0).clamp(-24.0, 24.0)).clamp(0.0, 1.0));
    (m, Some("CleanQ's band is its own design (the filter's slope option is not read)".into()))
}

/// Dyn3 Compressor/Limiter as Compressor/Limiter: threshold, ratio, attack, release, knee,
/// make-up gain, side-chain high-pass.
fn dyn3_to_compressor(ins: &PInsert) -> (Insert, Option<String>) {
    let mut m = Insert::new("compressor");
    let mut set = |k: &str, v: Option<f64>, lo: f64, hi: f64| {
        if let Some(v) = v {
            m.params.insert(k.into(), v.clamp(lo, hi) as f32);
        }
    };
    set("threshold", num(ins, "d_sldC"), -60.0, 0.0);
    set("ratio", num(ins, "d_sld2"), 1.0, 100.0);
    set("attack", num(ins, "d_sld1").map(|s| s * 1000.0), 0.01, 300.0);
    set("release", num(ins, "d_sld3").map(|s| s * 1000.0), 5.0, 3000.0);
    set("knee", num(ins, "d_sldD"), 0.0, 24.0);
    set("makeup", num(ins, "d_sldG"), 0.0, 40.0);
    set("sc_hpf_on", ins.params.get("l_LoFE").and_then(Value::as_i64).map(|v| f64::from(u8::from(v != 0))), 0.0, 1.0);
    set("sc_hpf_freq", num(ins, "d_LoFF"), 20.0, 500.0);
    (m, Some("the settings are copied; the two compressors' detectors and curves are not the same".into()))
}

/// Trim as Trim: its gain and phase invert (the reader's gain curve is within 0.06 dB).
fn trim_to_trim(ins: &PInsert) -> (Insert, Option<String>) {
    let mut m = Insert::new("trim");
    if let Some(g) = num(ins, "gain_db") {
        m.params.insert("gain".into(), g.clamp(-144.0, 12.0) as f32);
    }
    if let Some(p) = ins.params.get("phase_invert").and_then(Value::as_bool) {
        m.params.insert("invert".into(), f32::from(u8::from(p)));
    }
    (m, Some("its gain is read from Pro Tools' control position (within 0.06 dB)".into()))
}

#[cfg(test)]
#[path = "ptx_import_tests.rs"]
mod tests;
