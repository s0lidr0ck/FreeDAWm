//! DAWproject → session. See the module docs in `mod.rs` for what maps.

use super::archive::{Archive, normalise};
use super::report::{Fate, Report};
use super::xml::Element;
use super::{PendingAuto, Unit, db_from, pan_from, parse_color, state_print};
use crate::open_job::{OpenPhase, OpenProgress};
use soundcraft_audio_io::FileFormat;
use soundcraft_model::{
    AutoParam, ChannelFormat, Clip, ForeignDevice, Insert, ModuleId, Playlist, Route, SEND_SLOTS, SendSlot, Session, SourceId, TrackId, TrackKind,
};
use soundcraft_time::{MAX_POSITION, SampleRate, Samples};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

/// Largest `project.xml` read.
const MAX_XML_BYTES: u64 = 256 << 20;
/// Largest embedded plugin state read.
const MAX_STATE_BYTES: u64 = 64 << 20;
/// Largest embedded audio file read.
const MAX_AUDIO_BYTES: u64 = 4 << 30;
/// Most clips one looped clip unrolls into.
const MAX_LOOP_COPIES: usize = 4096;
/// Most notes or controller events read into one clip.
const MAX_EVENTS: usize = 1_000_000;
/// Most automation points read into one lane.
const MAX_POINTS: usize = 200_000;
/// Deepest clip-in-clip nesting followed.
const MAX_CLIP_NESTING: usize = 16;
/// Ticks per quarter note (beat).
const TPQ: f64 = soundcraft_time::TICKS_PER_QUARTER as f64;

/// Which plugins are installed: `Some(true/false)` from the scan caches, `None` when unknown
/// (never scanned).
pub type Installed = Box<dyn Fn(&str) -> Option<bool> + Send>;

/// The default lookup: the VST3 and CLAP scan caches (read, never a scan).
pub fn installed_from_caches() -> Installed {
    let vst3: Option<HashSet<String>> = soundcraft_vst3_host::cached().map(|v| v.into_iter().map(|d| d.id.to_ascii_uppercase()).collect());
    let clap: Option<HashSet<String>> = soundcraft_clap_host::cached().map(|v| v.into_iter().map(|d| d.id).collect());
    Box::new(move |id: &str| {
        if id.starts_with("vst3:") {
            vst3.as_ref().map(|s| s.contains(&id.to_ascii_uppercase()))
        } else if id.starts_with("clap:") {
            clap.as_ref().map(|s| s.contains(id))
        } else {
            soundcraft_dsp::plugin_info(id).map(|_| true)
        }
    })
}

/// The result of an import.
pub struct Imported {
    pub session: Session,
    pub report: Report,
    /// Plugin parameter automation waiting for the plugin's parameter ranges (see
    /// [`super::resolve_pending`]).
    pub pending: Vec<PendingAuto>,
}

/// What an automation target names.
#[derive(Debug, Clone)]
enum Target {
    Volume(TrackId, Unit2),
    Pan(TrackId, Unit2),
    Mute(TrackId),
    SendLevel(TrackId, u8, Unit2),
    SendPan(TrackId, u8, Unit2),
    SendEnable(TrackId, u8),
    Plugin {
        track: TrackId,
        module: ModuleId,
        plugin: String,
        key: String,
        name: String,
        unit: Unit2,
    },
    /// A parameter of something whose automation can't be carried (an instrument slot, a
    /// device's on/off switch), with what to call it.
    Unsupported(TrackId, String),
}

/// A parameter's unit (`unit` attribute), as given.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Unit2(pub String);

/// A channel read from the file.
#[derive(Debug, Clone)]
struct Chan {
    track: TrackId,
    /// The bus feeding the track (busses, effect returns, folders).
    bus: Option<soundcraft_model::BusId>,
    master: bool,
}

/// A clip window: content times map to absolute timeline times (`abs = offset + t`, in `unit`),
/// and only `[lo, hi)` shows.
#[derive(Debug, Clone, Copy)]
struct Frame {
    unit: Unit,
    offset: f64,
    lo: f64,
    hi: f64,
}

/// One automation point as read.
#[derive(Debug, Clone, Copy)]
struct Pt {
    time: f64,
    value: f64,
    hold: bool,
}

struct Importer<'p, 'a, R: Read + Seek> {
    archive: &'a mut Archive<R>,
    dir: Option<PathBuf>,
    s: Session,
    report: Report,
    installed: Installed,
    tracks: HashMap<&'p str, TrackId>,
    names: HashMap<TrackId, String>,
    chans: HashMap<&'p str, Chan>,
    targets: HashMap<&'p str, Target>,
    timelines: HashMap<&'p str, &'p Element>,
    sources: HashMap<String, Result<SourceId, String>>,
    clips_seen: HashMap<TrackId, usize>,
    pending: Vec<PendingAuto>,
    tempo_id: Option<&'p str>,
    tsig_id: Option<&'p str>,
    audio_clips: usize,
    midi_clips: usize,
    notes: usize,
    lanes: usize,
    markers: usize,
}

/// Imports the archive. `name` is the session's name (the file's stem), `file` the file name for
/// the report, `dir` the folder external files are relative to.
pub fn import<R: Read + Seek>(
    archive: &mut Archive<R>,
    name: &str,
    file: &str,
    dir: Option<&Path>,
    installed: Installed,
    progress: &mut dyn FnMut(OpenProgress),
) -> Result<Imported, String> {
    progress(OpenProgress::new(OpenPhase::Reading, 0, 0, file));
    let text = archive.read("project.xml", MAX_XML_BYTES)?;
    let text = String::from_utf8(text).map_err(|_| "project.xml is not UTF-8 text".to_string())?;
    let project = Element::parse(text.trim_start_matches('\u{feff}')).map_err(|e| format!("project.xml: {e}"))?;
    if project.name != "Project" {
        return Err(format!("project.xml: the root is <{}>, not <Project>", project.name));
    }
    let meta = archive
        .read("metadata.xml", 1 << 20)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|t| Element::parse(t.trim_start_matches('\u{feff}')).ok());

    let rate = session_rate(&project);
    let mut s = Session::new(name, rate);
    s.tracks.clear();
    let mut report = Report { file: file.to_string(), ..Report::default() };
    if let Some(app) = project.child("Application") {
        report.application = format!("{} {}", app.attr("name").unwrap_or(""), app.attr("version").unwrap_or("")).trim().to_string();
    }
    if let Some(m) = &meta
        && let Some(c) = m.child("Comment").map(|c| c.text.trim()).filter(|c| !c.is_empty())
    {
        s.comments = c.to_string();
    }
    let mut im = Importer {
        archive,
        dir: dir.map(Path::to_path_buf),
        s,
        report,
        installed,
        tracks: HashMap::new(),
        names: HashMap::new(),
        chans: HashMap::new(),
        targets: HashMap::new(),
        timelines: HashMap::new(),
        sources: HashMap::new(),
        clips_seen: HashMap::new(),
        pending: Vec::new(),
        tempo_id: None,
        tsig_id: None,
        audio_clips: 0,
        midi_clips: 0,
        notes: 0,
        lanes: 0,
        markers: 0,
    };
    im.index_timelines(&project, 0);
    im.transport(&project);
    im.audio(&project, progress)?;
    if let Some(st) = project.child("Structure") {
        im.structure(st);
    }
    if let Some(arr) = project.child("Arrangement") {
        im.arrangement(arr);
    }
    if let Some(sc) = project.child("Scenes").filter(|s| !s.children.is_empty()) {
        let n = sc.children.len();
        im.report.add(
            Fate::Dropped,
            "",
            format!("{n} scene{}", if n == 1 { "" } else { "s" }),
            "the clip launcher (scenes and clip slots) is not supported",
        );
    }
    im.finish();
    Ok(Imported { session: im.s, report: im.report, pending: im.pending })
}

/// The session rate: the rate most of the project's audio files are at, else 48 kHz.
fn session_rate(project: &Element) -> SampleRate {
    let mut counts: HashMap<i64, usize> = HashMap::new();
    let mut stack = vec![project];
    let mut seen = 0usize;
    while let Some(e) = stack.pop() {
        seen += 1;
        if seen > 5_000_000 {
            break;
        }
        if e.name == "Audio"
            && let Some(r) = e.int("sampleRate")
        {
            *counts.entry(r).or_default() += 1;
        }
        stack.extend(e.children.iter());
    }
    let best = counts.into_iter().max_by_key(|(r, n)| (*n, *r)).map(|(r, _)| r);
    best.and_then(|r| u32::try_from(r).ok()).and_then(|r| SampleRate::new(r).ok()).unwrap_or(SampleRate::HZ_48000)
}

fn unit_of(e: &Element, key: &str) -> Option<Unit> {
    match e.attr(key)? {
        "beats" => Some(Unit::Beats),
        "seconds" => Some(Unit::Seconds),
        _ => None,
    }
}

fn clamp_pos(v: f64) -> Samples {
    if v.is_finite() { (v.round() as Samples).clamp(0, MAX_POSITION) } else { 0 }
}

impl<'p, R: Read + Seek> Importer<'p, '_, R> {
    fn sr(&self) -> f64 {
        self.s.sample_rate.as_f64()
    }

    /// An absolute time in `unit` as fractional samples.
    fn samples(&self, t: f64, unit: Unit) -> f64 {
        match unit {
            Unit::Beats => self.s.tempo.ticks_f_to_samples_f(t * TPQ, self.s.sample_rate),
            Unit::Seconds => t * self.sr(),
        }
    }

    /// Fractional samples as beats.
    fn beats_at(&self, samples: f64) -> f64 {
        let s = if samples.is_finite() { samples.round() as Samples } else { 0 };
        self.s.tempo.samples_to_ticks_f(s, self.s.sample_rate) / TPQ
    }

    fn track_name(&self, t: TrackId) -> String {
        self.names.get(&t).cloned().unwrap_or_default()
    }

    /// Every element with an `id`, by id (alias clips name their content by it).
    fn index_timelines(&mut self, e: &'p Element, depth: usize) {
        if depth > 200 {
            return;
        }
        if let Some(id) = e.attr("id") {
            self.timelines.entry(id).or_insert(e);
        }
        for c in &e.children {
            self.index_timelines(c, depth + 1);
        }
    }

    // ---- transport ---------------------------------------------------------------------------

    fn transport(&mut self, project: &'p Element) {
        if let Some(tr) = project.child("Transport") {
            if let Some(t) = tr.child("Tempo") {
                self.tempo_id = t.attr("id");
                if let Some(bpm) = t.num("value")
                    && self.s.tempo.set_tempo(0, bpm).is_err()
                {
                    self.report.add(Fate::Changed, "", format!("Tempo {bpm} BPM"), "out of range: 120 BPM used");
                }
            }
            if let Some(ts) = tr.child("TimeSignature") {
                self.tsig_id = ts.attr("id");
                let (n, d) = (ts.int("numerator").unwrap_or(4), ts.int("denominator").unwrap_or(4));
                if self.s.tempo.set_meter(0, u32::try_from(n).unwrap_or(4), u32::try_from(d).unwrap_or(4)).is_err() {
                    self.report.add(Fate::Changed, "", format!("Time signature {n}/{d}"), "not supported: 4/4 used");
                }
            }
        }
        let Some(arr) = project.child("Arrangement") else { return };
        // Tempo automation: the arrangement's own lane, and any lane aimed at the tempo.
        let mut lanes: Vec<(&Element, Unit)> = Vec::new();
        if let Some(t) = arr.child("TempoAutomation") {
            lanes.push((t, unit_of(t, "timeUnit").unwrap_or(Unit::Beats)));
        }
        if let (Some(id), Some(l)) = (self.tempo_id, arr.child("Lanes")) {
            collect_points_for(l, id, unit_of(l, "timeUnit").unwrap_or(Unit::Beats), &mut lanes, 0);
        }
        let mut tempo_points = 0usize;
        for (lane, unit) in lanes {
            let pts = read_points(lane);
            for (i, p) in pts.iter().enumerate() {
                let tick = match unit {
                    Unit::Beats => p.time * TPQ,
                    Unit::Seconds => self.s.tempo.samples_to_ticks_f(clamp_pos(p.time * self.sr()), self.s.sample_rate),
                };
                let tick = if tick.is_finite() { (tick.round() as i64).clamp(0, MAX_POSITION) } else { 0 };
                if self.s.tempo.set_tempo(tick, p.value).is_err() {
                    self.report.add(Fate::Changed, "", format!("Tempo change to {} BPM", p.value), "out of range: left out");
                    continue;
                }
                tempo_points += 1;
                let ramps = !p.hold && pts.get(i + 1).is_some_and(|n| (n.value - p.value).abs() > 1e-9);
                self.s.tempo.set_ramp(tick, ramps);
            }
        }
        if tempo_points > 0 {
            self.report.carried.push(format!("tempo map ({tempo_points} changes)"));
        }
        if let Some(ts) = arr.child("TimeSignatureAutomation") {
            let unit = unit_of(ts, "timeUnit").unwrap_or(Unit::Beats);
            let mut n = 0usize;
            for p in ts.children.iter().filter(|c| c.name == "TimeSignaturePoint").take(10_000) {
                let Some(t) = p.num("time") else { continue };
                let tick = match unit {
                    Unit::Beats => t * TPQ,
                    Unit::Seconds => self.s.tempo.samples_to_ticks_f(clamp_pos(t * self.sr()), self.s.sample_rate),
                };
                let tick = if tick.is_finite() { (tick.round() as i64).clamp(0, MAX_POSITION) } else { 0 };
                let (num, den) = (p.int("numerator").unwrap_or(4), p.int("denominator").unwrap_or(4));
                match self.s.tempo.set_meter(tick, u32::try_from(num).unwrap_or(0), u32::try_from(den).unwrap_or(0)) {
                    Ok(()) => n += 1,
                    Err(_) => self.report.add(Fate::Dropped, "", format!("Time signature {num}/{den}"), "not supported"),
                }
            }
            if n > 0 {
                self.report.carried.push(format!("{n} time signature changes"));
            }
        }
    }

    // ---- audio files -------------------------------------------------------------------------

    /// Decodes every audio file the project names (embedded or external), once each.
    fn audio(&mut self, project: &'p Element, progress: &mut dyn FnMut(OpenProgress)) -> Result<(), String> {
        let mut files: Vec<(String, bool)> = Vec::new();
        let mut stack = vec![project];
        while let Some(e) = stack.pop() {
            if e.name == "Audio"
                && let Some(f) = e.child("File")
                && let Some(p) = f.attr("path")
            {
                let key = (p.to_string(), f.flag("external").unwrap_or(false));
                if !files.contains(&key) {
                    files.push(key);
                }
            }
            stack.extend(e.children.iter().rev());
        }
        let total = files.len();
        for (i, (path, external)) in files.into_iter().enumerate() {
            let shown = Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
            progress(OpenProgress::new(OpenPhase::Audio, i, total, shown.clone()));
            let r = self.load_audio(&path, external, &shown);
            self.sources.insert(file_key(&path, external), r);
        }
        progress(OpenProgress::new(OpenPhase::Waveforms, total, total, ""));
        Ok(())
    }

    fn load_audio(&mut self, path: &str, external: bool, shown: &str) -> Result<SourceId, String> {
        let bytes = self.file_bytes(path, external, MAX_AUDIO_BYTES)?;
        let ext = Path::new(path).extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
        let (info, buf) = soundcraft_audio_io::decode(&bytes, ext.as_deref()).map_err(|e| format!("{shown}: {e}"))?;
        drop(bytes);
        if buf.frames() == 0 {
            return Err(format!("{shown}: no audio"));
        }
        let buf = crate::io::at_rate(buf, buf_rate(&info), 1 << 30).ok_or_else(|| format!("{shown}: too large"))?;
        let format = if info.format == FileFormat::Wav { FileFormat::Wav } else { info.format };
        Ok(crate::io::add_source(&mut self.s, shown, buf, None, format))
    }

    /// The bytes of a file the project names: an archive entry, or (external) a file beside the
    /// project or at an absolute path.
    fn file_bytes(&mut self, path: &str, external: bool, max: u64) -> Result<Vec<u8>, String> {
        if !external || self.archive.contains(path) {
            if let Ok(b) = self.archive.read(path, max) {
                return Ok(b);
            }
            if !external {
                return Err(format!("{path}: not in the archive"));
            }
        }
        if cfg!(target_arch = "wasm32") {
            return Err(format!("{path}: an external file (not in the archive)"));
        }
        let p = Path::new(path);
        let full = if p.is_absolute() { p.to_path_buf() } else { self.dir.clone().unwrap_or_default().join(p) };
        crate::io::read_file_max(&full, max).map_err(|e| e.to_string())
    }

    // ---- structure ---------------------------------------------------------------------------

    fn structure(&mut self, st: &'p Element) {
        let mut later: Vec<(TrackId, &'p Element)> = Vec::new();
        for c in &st.children {
            match c.name.as_str() {
                "Track" => self.track(c, None, &mut later, 0),
                "Channel" => {
                    let kind = channel_kind(c, None);
                    let name = c.attr("name").unwrap_or(if kind == TrackKind::Master { "Master" } else { "Bus" });
                    let t = self.new_track(kind, c, name, c.attr("color"), None);
                    if let Some(id) = c.attr("id") {
                        self.tracks.insert(id, t);
                    }
                    self.channel(t, c, kind, &mut later);
                }
                _ => {}
            }
        }
        // Outputs and sends name channels anywhere in the structure: resolved once all exist.
        for (t, ch) in later {
            self.routing(t, ch);
        }
    }

    fn track(&mut self, el: &'p Element, folder: Option<TrackId>, later: &mut Vec<(TrackId, &'p Element)>, depth: usize) {
        if depth > 32 {
            self.report.add(Fate::Dropped, "", el.attr("name").unwrap_or("Track"), "folders nested deeper than 32 levels");
            return;
        }
        let ch = el.child("Channel");
        let kind = track_kind(el, ch);
        let name = el.attr("name").unwrap_or(kind.default_name());
        let t = self.new_track(kind, ch.unwrap_or(el), name, el.attr("color"), folder);
        if let Some(id) = el.attr("id") {
            self.tracks.insert(id, t);
        }
        if let Some(ch) = ch {
            self.channel(t, ch, kind, later);
        }
        if el.flag("loaded") == Some(false) {
            self.report.add(Fate::Changed, name, "Track", "was not loaded (deactivated) in the other application: imported inactive");
            if let Some(tr) = self.s.track_mut(t) {
                tr.inactive = true;
            }
        }
        for c in el.children_named("Track") {
            self.track(c, Some(t), later, depth + 1);
        }
    }

    fn new_track(&mut self, kind: TrackKind, ch: &'p Element, name: &str, color: Option<&str>, folder: Option<TrackId>) -> TrackId {
        let format = match ch.int("audioChannels") {
            Some(1) => ChannelFormat::Mono,
            Some(2) | None => ChannelFormat::Stereo,
            Some(n) => ChannelFormat::for_channels(usize::try_from(n).unwrap_or(2).clamp(1, 16)),
        };
        let format = if kind == TrackKind::Midi { ChannelFormat::Mono } else { format };
        let t = self.s.add_track(kind, format, Some(name));
        // The name as written (`add_track` numbers default names like "Audio"), unless taken.
        if !self.s.tracks.iter().any(|x| x.name == name)
            && let Some(tr) = self.s.track_mut(t)
        {
            tr.name = name.chars().take(soundcraft_model::MAX_NAME_CHARS).collect();
            if let Some(pl) = tr.playlists.first_mut() {
                pl.name.clone_from(&tr.name);
            }
        }
        self.names.insert(t, name.to_string());
        let needs_bus = matches!(kind, TrackKind::Folder | TrackKind::Aux);
        let bus = needs_bus.then(|| {
            let base = format!("{name} Bus");
            let mut bus_name = base.clone();
            let mut n = 2;
            while self.s.bus_by_name(&bus_name).is_some() {
                bus_name = format!("{base} {n}");
                n += 1;
            }
            self.s.add_bus(&bus_name, if format.channels() >= 2 { format } else { ChannelFormat::Stereo })
        });
        if let Some(tr) = self.s.track_mut(t) {
            if let Some(c) = color.and_then(parse_color) {
                tr.color = c;
            }
            tr.folder = folder;
            if let Some(b) = bus {
                tr.mixer.input = Route::Bus(b);
            }
            if kind == TrackKind::Instrument {
                tr.mixer.input = Route::Midi(soundcraft_model::MidiIn::all());
            }
            if let Some(c) = ch.attr("comment").filter(|c| !c.is_empty()) {
                tr.comments = c.to_string();
            }
        }
        if let Some(id) = ch.attr("id") {
            self.chans.insert(id, Chan { track: t, bus, master: kind == TrackKind::Master });
        }
        t
    }

    /// The channel's mixer settings and devices (outputs and sends come later, in `routing`).
    fn channel(&mut self, t: TrackId, ch: &'p Element, kind: TrackKind, later: &mut Vec<(TrackId, &'p Element)>) {
        let name = self.track_name(t);
        if let Some(id) = ch.attr("id") {
            self.chans.entry(id).or_insert(Chan { track: t, bus: None, master: kind == TrackKind::Master });
            self.tracks.entry(id).or_insert(t);
        }
        let stereo = self.s.track(t).is_some_and(|tr| tr.format == ChannelFormat::Stereo);
        if let Some(v) = ch.child("Volume") {
            let unit = Unit2(v.attr("unit").unwrap_or("linear").to_string());
            if let Some(x) = v.num("value") {
                let db = db_from(x, &unit.0);
                if let Some(tr) = self.s.track_mut(t) {
                    tr.mixer.volume_db = db;
                }
            }
            if let Some(id) = v.attr("id") {
                self.targets.insert(id, Target::Volume(t, unit));
            }
        }
        if let Some(p) = ch.child("Pan") {
            let unit = Unit2(p.attr("unit").unwrap_or("normalized").to_string());
            if let Some(x) = p.num("value") {
                let pan = pan_from(x, &unit.0);
                if let Some(tr) = self.s.track_mut(t) {
                    tr.mixer.pan = if stereo {
                        balance(pan).to_vec()
                    } else if tr.mixer.pan.len() == 1 {
                        vec![pan]
                    } else {
                        tr.mixer.pan.clone()
                    };
                }
            }
            if let Some(id) = p.attr("id") {
                self.targets.insert(id, Target::Pan(t, unit));
            }
        }
        if let Some(m) = ch.child("Mute") {
            if let Some(tr) = self.s.track_mut(t) {
                tr.mixer.mute = m.flag("value").unwrap_or(false);
            }
            if let Some(id) = m.attr("id") {
                self.targets.insert(id, Target::Mute(t));
            }
        }
        if ch.flag("solo") == Some(true)
            && let Some(tr) = self.s.track_mut(t)
        {
            tr.mixer.solo = true;
        }
        if let Some(devs) = ch.child("Devices") {
            for d in &devs.children {
                self.device(t, d, &name, kind);
            }
        }
        later.push((t, ch));
    }

    /// Output and sends, once every channel is known.
    fn routing(&mut self, t: TrackId, ch: &'p Element) {
        let name = self.track_name(t);
        let kind = self.s.track(t).map(|tr| tr.kind).unwrap_or_default();
        if kind != TrackKind::Master {
            let out = match ch.attr("destination") {
                None => Route::Main,
                Some(d) => match self.chans.get(d).cloned() {
                    Some(c) if c.master => Route::Main,
                    Some(c) if c.track == t => Route::Main,
                    Some(Chan { bus: Some(b), .. }) => Route::Bus(b),
                    Some(c) => {
                        let to = self.track_name(c.track);
                        self.report.add(
                            Fate::Changed,
                            &name,
                            format!("Output to {to}"),
                            "a track can only output to a bus or folder here: sent to the main mix",
                        );
                        Route::Main
                    }
                    None => {
                        self.report.add(Fate::Changed, &name, "Output", "names a channel that is not in the file: sent to the main mix");
                        Route::Main
                    }
                },
            };
            if let Some(tr) = self.s.track_mut(t) {
                tr.mixer.output = out;
            }
        }
        let Some(sends) = ch.child("Sends") else { return };
        let mut slot = 0usize;
        for send in sends.children_named("Send") {
            let to = send.attr("destination").and_then(|d| self.chans.get(d).cloned());
            let Some(Chan { bus: Some(bus), track: to_track, .. }) = to else {
                self.report.add(Fate::Dropped, &name, "Send", "its destination is not a bus here");
                continue;
            };
            if slot >= SEND_SLOTS {
                self.report.add(Fate::Dropped, &name, format!("Send to {}", self.track_name(to_track)), format!("more than {SEND_SLOTS} sends"));
                continue;
            }
            let i = u8::try_from(slot).unwrap_or(0);
            let mut sl = SendSlot::new(Route::Bus(bus));
            sl.pre_fader = send.attr("type") == Some("pre");
            if let Some(v) = send.child("Volume") {
                let unit = Unit2(v.attr("unit").unwrap_or("linear").to_string());
                sl.level_db = v.num("value").map_or(0.0, |x| db_from(x, &unit.0));
                if let Some(id) = v.attr("id") {
                    self.targets.insert(id, Target::SendLevel(t, i, unit));
                }
            } else {
                sl.level_db = 0.0;
            }
            if let Some(p) = send.child("Pan") {
                let unit = Unit2(p.attr("unit").unwrap_or("normalized").to_string());
                if let Some(x) = p.num("value") {
                    sl.pan = pan_from(x, &unit.0);
                    sl.follow_main_pan = false;
                }
                if let Some(id) = p.attr("id") {
                    self.targets.insert(id, Target::SendPan(t, i, unit));
                }
            }
            if let Some(en) = send.child("Enable") {
                sl.mute = en.flag("value") == Some(false);
                if let Some(id) = en.attr("id") {
                    self.targets.insert(id, Target::SendEnable(t, i));
                }
            }
            if let Some(tr) = self.s.track_mut(t)
                && let Some(s) = tr.mixer.sends.get_mut(slot)
            {
                *s = Some(sl);
            }
            slot += 1;
        }
    }

    // ---- devices -----------------------------------------------------------------------------

    fn device(&mut self, t: TrackId, d: &'p Element, track_name: &str, kind: TrackKind) {
        const KINDS: [&str; 10] =
            ["Device", "Vst2Plugin", "Vst3Plugin", "ClapPlugin", "BuiltinDevice", "Equalizer", "Compressor", "NoiseGate", "Limiter", "AuPlugin"];
        if !KINDS.contains(&d.name.as_str()) {
            return;
        }
        let role = d.attr("deviceRole").unwrap_or("audioFX");
        let dname = d.attr("deviceName").unwrap_or("Device").to_string();
        let ins = self.device_insert(d, &dname, track_name);
        let instrument_slot = role == "instrument" && kind == TrackKind::Instrument && self.s.track(t).is_some_and(|tr| tr.instrument.is_none());
        let module = if instrument_slot {
            if let Some(tr) = self.s.track_mut(t) {
                tr.instrument = Some(ins);
            }
            None
        } else {
            if role == "noteFX" {
                self.report.add(Fate::Placeholder, track_name, format!("Note FX {dname}"), "MIDI effects are not hosted: kept as an inactive slot");
            } else if role == "instrument" {
                self.report.add(
                    Fate::Placeholder,
                    track_name,
                    format!("Instrument {dname}"),
                    "a second instrument on one track: kept as an inactive slot",
                );
            }
            let mut ins = ins;
            if role != "audioFX" && role != "analyzer" {
                ins.active = false;
            }
            match self.s.track_mut(t).map(|tr| tr.mixer.add_module(ins, None)) {
                Some(Ok(m)) => Some(m),
                _ => {
                    self.report.add(
                        Fate::Dropped,
                        track_name,
                        dname.clone(),
                        format!("more than {} modules on the track", soundcraft_model::MAX_MODULES),
                    );
                    return;
                }
            }
        };
        let plugin = match module {
            Some(m) => self.s.track(t).and_then(|tr| tr.mixer.module(m)).map(|i| i.plugin.clone()),
            None => self.s.track(t).and_then(|tr| tr.instrument.as_ref()).map(|i| i.plugin.clone()),
        }
        .unwrap_or_default();
        if let Some(en) = d.child("Enabled").and_then(|e| e.attr("id")) {
            self.targets.insert(en, Target::Unsupported(t, format!("{dname} on/off")));
        }
        if let Some(params) = d.child("Parameters") {
            for p in &params.children {
                let Some(id) = p.attr("id") else { continue };
                let pname = p.attr("name").unwrap_or("parameter").to_string();
                let target = match (module, p.attr("parameterID")) {
                    (Some(module), key) => {
                        let key = match key {
                            Some(k) => k.trim().to_string(),
                            None => pname.clone(),
                        };
                        Target::Plugin {
                            track: t,
                            module,
                            plugin: plugin.clone(),
                            key,
                            name: format!("{dname} {pname}"),
                            unit: Unit2(p.attr("unit").unwrap_or("normalized").to_string()),
                        }
                    }
                    (None, _) => Target::Unsupported(t, format!("{dname} {pname} (instrument)")),
                };
                self.targets.insert(id, target);
            }
        }
    }

    /// The module for a device element: the plugin itself when it can be hosted here (with its
    /// saved state), else an inactive placeholder keeping the element and its state file.
    fn device_insert(&mut self, d: &Element, dname: &str, track_name: &str) -> Insert {
        let device_id = d.attr("deviceID").unwrap_or("").trim().to_string();
        let enabled = d.child("Enabled").and_then(|e| e.flag("value")).unwrap_or(true);
        let state_ref = d.child("State").and_then(|s| s.attr("path").map(|p| (p.to_string(), s.flag("external").unwrap_or(false))));
        let state_bytes = match &state_ref {
            Some((p, ext)) => match self.file_bytes(p, *ext, MAX_STATE_BYTES) {
                Ok(b) => Some(b),
                Err(e) => {
                    self.report.add(
                        Fate::Changed,
                        track_name,
                        dname.to_string(),
                        format!("its state file is missing ({e}): loads with its defaults"),
                    );
                    None
                }
            },
            None => None,
        };
        let foreign = |ins: &Insert| ForeignDevice {
            xml: d.to_xml(),
            name: dname.to_string(),
            state: state_bytes.as_deref().map(soundcraft_model::b64::encode),
            state_print: state_print(ins.state.as_deref()),
        };
        // FreeDAWm's own modules, written by our export: the module comes back as it was.
        if let Some(own) = device_id.strip_prefix("freedawm:") {
            let back = state_bytes
                .as_deref()
                .and_then(|b| serde_json::from_slice::<Insert>(b).ok())
                .filter(|i| i.plugin == own && (self.installed)(own) == Some(true));
            if let Some(mut ins) = back {
                ins.id = ModuleId::default();
                ins.bypass = !enabled;
                ins.foreign = None;
                return ins;
            }
        }
        let (plugin, what) = match d.name.as_str() {
            "Vst3Plugin" if device_id.len() == 32 && device_id.bytes().all(|b| b.is_ascii_hexdigit()) => {
                (Some(self.vst3_id(&device_id, state_bytes.as_deref())), format!("VST3 {dname}"))
            }
            "ClapPlugin" if !device_id.is_empty() && !device_id.contains(['\0', '\n']) => {
                (Some(format!("clap:{device_id}")), format!("CLAP {dname}"))
            }
            "Vst3Plugin" => (None, format!("VST3 {dname}")),
            "ClapPlugin" => (None, format!("CLAP {dname}")),
            "Vst2Plugin" => (None, format!("VST2 {dname}")),
            "AuPlugin" => (None, format!("Audio Unit {dname}")),
            _ => (None, dname.to_string()),
        };
        let Some(plugin) = plugin else {
            let why = match d.name.as_str() {
                "Vst2Plugin" => "VST2 plugins are not hosted".to_string(),
                "AuPlugin" => "Audio Units from a DAWproject file are not hosted".to_string(),
                "Vst3Plugin" | "ClapPlugin" => "no plugin id in the file".to_string(),
                _ => format!(
                    "a built-in device of {}",
                    if self.report.application.is_empty() { "the other application" } else { &self.report.application }
                ),
            };
            let mut ins = Insert::new(format!("dawproject:{}", dname.replace([':', '.', '/'], " ")));
            ins.active = false;
            ins.bypass = !enabled;
            ins.preset = dname.to_string();
            ins.foreign = Some(foreign(&ins));
            self.report.add(Fate::Placeholder, track_name, what, why);
            return ins;
        };
        let mut ins = Insert::new(plugin.clone());
        ins.bypass = !enabled;
        if let Some(bytes) = &state_bytes {
            let state = if plugin.starts_with("vst3:") {
                match soundcraft_vst3_host::vstpreset::read_vstpreset(bytes) {
                    Ok(p) => p.state_blob(),
                    Err(e) => {
                        self.report.add(Fate::Changed, track_name, what.clone(), format!("its state ({e}) is not usable: loads with its defaults"));
                        None
                    }
                }
            } else {
                Some(bytes.clone())
            };
            if let Some(st) = state {
                ins.set_state_bytes(&st);
            }
        }
        match (self.installed)(&plugin) {
            Some(false) => {
                ins.active = false;
                ins.preset = dname.to_string();
                self.report.add(Fate::Placeholder, track_name, what, "not installed: kept as an inactive slot with its state");
            }
            Some(true) => self.report.plugins.push(format!("{track_name}: {dname}")),
            None => self.report.plugins.push(format!("{track_name}: {dname} (if installed)")),
        }
        ins.foreign = Some(foreign(&ins));
        ins
    }

    /// FreeDAWm's id for a DAWproject VST3 class id: the form the scan cache knows, else this
    /// platform's (DAWproject and `.vstpreset` files write the canonical text; Windows lays the
    /// bytes out in GUID order, so its ids swap the first eight bytes).
    fn vst3_id(&self, device_id: &str, state: Option<&[u8]>) -> String {
        use soundcraft_vst3_host::vstpreset::other_class_form;
        let text = device_id.to_ascii_uppercase();
        let other = other_class_form(&text).unwrap_or_else(|| text.clone()).to_ascii_uppercase();
        let (native, foreign) = if cfg!(windows) { (other, text) } else { (text, other) };
        for c in [&native, &foreign] {
            if (self.installed)(&format!("vst3:{c}")) == Some(true) {
                return format!("vst3:{c}");
            }
        }
        let _ = state;
        format!("vst3:{native}")
    }

    // ---- arrangement -------------------------------------------------------------------------

    fn arrangement(&mut self, arr: &'p Element) {
        if let Some(l) = arr.child("Lanes") {
            let unit = unit_of(l, "timeUnit").unwrap_or(Unit::Beats);
            let track = l.attr("track").and_then(|t| self.tracks.get(t).copied());
            self.lanes(l, track, unit, 0);
        }
        if let Some(m) = arr.child("Markers") {
            self.markers(m, unit_of(m, "timeUnit").unwrap_or(Unit::Beats));
        }
    }

    fn lanes(&mut self, el: &'p Element, track: Option<TrackId>, unit: Unit, depth: usize) {
        if depth > 64 {
            return;
        }
        for c in &el.children {
            let unit = unit_of(c, "timeUnit").unwrap_or(unit);
            let track = c.attr("track").and_then(|t| self.tracks.get(t).copied()).or(track);
            match c.name.as_str() {
                "Lanes" => self.lanes(c, track, unit, depth + 1),
                "Clips" => match track {
                    Some(t) => self.clips(c, t, unit),
                    None => self.report.add(Fate::Dropped, "", format!("{} clips", c.children.len()), "on a lane that names no track"),
                },
                "Points" => self.points(c, track, unit),
                "Notes" | "Audio" | "Warps" => match track {
                    // Content straight on a lane: one clip around it.
                    Some(t) => {
                        let wrapper = Element::new("Clip").with("time", 0.0);
                        let frame = Frame { unit, offset: 0.0, lo: f64::NEG_INFINITY, hi: f64::INFINITY };
                        let pl = self.playlist_for(t);
                        let mut out = Vec::new();
                        self.clip_with(&wrapper, Some(c), t, frame, &mut out, 0);
                        self.place(t, pl, out);
                    }
                    None => self.report.add(Fate::Dropped, "", c.name.clone(), "on a lane that names no track"),
                },
                "markers" | "Markers" => self.markers(c, unit),
                "ClipSlot" => self.report.add(
                    Fate::Dropped,
                    &track.map(|t| self.track_name(t)).unwrap_or_default(),
                    "Clip slot",
                    "the clip launcher is not supported",
                ),
                "Video" => self.report.add(
                    Fate::Dropped,
                    &track.map(|t| self.track_name(t)).unwrap_or_default(),
                    "Video",
                    "video in DAWproject files is not imported",
                ),
                _ => {}
            }
        }
    }

    /// The playlist a new `Clips` lane of `t` fills: the first is the active one, each further
    /// lane is a new playlist (a take lane).
    fn playlist_for(&mut self, t: TrackId) -> usize {
        let n = self.clips_seen.entry(t).or_insert(0);
        let k = *n;
        *n += 1;
        if k > 0
            && let Some(tr) = self.s.track_mut(t)
        {
            let name = format!("{}.{}", tr.name, k + 1);
            tr.playlists.push(Playlist::new(name));
            return tr.playlists.len() - 1;
        }
        0
    }

    fn place(&mut self, t: TrackId, playlist: usize, clips: Vec<Clip>) {
        let Some(tr) = self.s.track_mut(t) else { return };
        if !tr.kind.has_playlist() {
            let name = tr.name.clone();
            self.report.add(Fate::Dropped, &name, format!("{} clips", clips.len()), "this kind of track holds no clips here");
            return;
        }
        let active = tr.active_playlist;
        tr.active_playlist = playlist.min(tr.playlists.len().saturating_sub(1));
        for c in clips {
            crate::edit::place_clip(&mut self.s, t, c);
        }
        if let Some(tr) = self.s.track_mut(t) {
            tr.active_playlist = active;
        }
    }

    fn clips(&mut self, el: &'p Element, t: TrackId, unit: Unit) {
        let pl = self.playlist_for(t);
        let frame = Frame { unit, offset: 0.0, lo: f64::NEG_INFINITY, hi: f64::INFINITY };
        let mut out = Vec::new();
        for c in el.children_named("Clip") {
            self.clip(c, t, frame, &mut out, 0);
        }
        self.place(t, pl, out);
    }

    fn clip(&mut self, c: &'p Element, t: TrackId, frame: Frame, out: &mut Vec<Clip>, depth: usize) {
        let content = match c.attr("reference") {
            Some(r) => match self.timelines.get(r).copied() {
                Some(e) => Some(e),
                None => {
                    self.report.add(Fate::Dropped, &self.track_name(t), clip_label(c), "an alias of content that is not in the file");
                    return;
                }
            },
            None => c.children.iter().find(|e| is_timeline(&e.name)),
        };
        self.clip_with(c, content, t, frame, out, depth);
    }

    /// One clip (`c`'s attributes) holding `content`, placed through `frame`.
    fn clip_with(&mut self, c: &Element, content: Option<&'p Element>, t: TrackId, frame: Frame, out: &mut Vec<Clip>, depth: usize) {
        let tname = self.track_name(t);
        let unit = frame.unit;
        let Some(time) = c.num("time").filter(|v| v.is_finite()) else {
            self.report.add(Fate::Dropped, &tname, clip_label(c), "no position");
            return;
        };
        let Some(content) = content else {
            if c.name == "Clip" {
                self.report.add(Fate::Dropped, &tname, clip_label(c), "an empty clip");
            }
            return;
        };
        let cunit = unit_of(c, "contentTimeUnit").or_else(|| unit_of(content, "timeUnit")).unwrap_or(unit);
        let play_start = c.num("playStart").filter(|v| v.is_finite()).unwrap_or(0.0);
        let play_stop = c.num("playStop").filter(|v| v.is_finite());
        let duration = match c.num("duration").filter(|v| v.is_finite() && *v > 0.0) {
            Some(d) => d,
            None => match (play_stop, content.num("duration")) {
                (Some(stop), _) if cunit == unit && stop > play_start => stop - play_start,
                (_, Some(d)) if content.name == "Audio" => {
                    let start = self.samples(frame.offset + time, unit);
                    let end = start + d * self.sr();
                    match unit {
                        Unit::Seconds => d,
                        Unit::Beats => self.beats_at(end) - self.beats_at(start),
                    }
                }
                _ => {
                    let notes_end =
                        content.children.iter().filter_map(|n| Some(n.num("time")? + n.num("duration").unwrap_or(0.0))).fold(0.0f64, f64::max);
                    if notes_end > 0.0 && cunit == unit {
                        notes_end - play_start
                    } else {
                        self.report.add(Fate::Dropped, &tname, clip_label(c), "no length");
                        return;
                    }
                }
            },
        };
        let abs_start = frame.offset + time;
        let abs_end = abs_start + duration;
        let vis_start = abs_start.max(frame.lo);
        let vis_end = abs_end.min(frame.hi);
        if vis_end <= vis_start {
            return;
        }
        let loop_ = match (c.num("loopStart"), c.num("loopEnd")) {
            (Some(a), Some(b)) if a.is_finite() && b.is_finite() && b > a => Some((a, b)),
            _ => None,
        };
        let spec = ClipSpec {
            name: c.attr("name").map(str::to_string),
            color: c.attr("color").and_then(parse_color),
            muted: c.flag("enable") == Some(false),
            unit,
            cunit,
            abs_start,
            vis_start,
            vis_end,
            play_start,
            play_stop,
            loop_,
            fade_unit: unit_of(c, "fadeTimeUnit").unwrap_or(unit),
            fade_in: c.num("fadeInTime").filter(|v| v.is_finite() && *v > 0.0),
            fade_out: c.num("fadeOutTime").filter(|v| v.is_finite() && *v > 0.0),
        };
        match content.name.as_str() {
            "Audio" | "Warps" => self.audio_clip(&spec, content, &[], t, out),
            "Notes" => self.midi_clip(&spec, Some(content), &[], t, out),
            "Points" => self.midi_clip(&spec, None, &[content], t, out),
            "Lanes" => {
                let audio = content.children.iter().find(|e| e.name == "Audio" || e.name == "Warps");
                let notes = content.children.iter().find(|e| e.name == "Notes");
                let points: Vec<&Element> = content.children.iter().filter(|e| e.name == "Points").collect();
                let clips = content.children.iter().find(|e| e.name == "Clips");
                if let Some(a) = audio {
                    self.audio_clip(&spec, a, &points, t, out);
                } else if notes.is_some() || !points.is_empty() {
                    self.midi_clip(&spec, notes, &points, t, out);
                } else if let Some(cl) = clips {
                    self.nested(&spec, cl, t, out, depth);
                }
            }
            "Clips" => self.nested(&spec, content, t, out, depth),
            other => self.report.add(Fate::Dropped, &tname, clip_label(c), format!("holds {other}, which is not imported")),
        }
    }

    /// A clip holding clips: each inner clip placed through the outer one's window.
    fn nested(&mut self, spec: &ClipSpec, clips: &'p Element, t: TrackId, out: &mut Vec<Clip>, depth: usize) {
        let tname = self.track_name(t);
        if depth >= MAX_CLIP_NESTING {
            self.report.add(Fate::Dropped, &tname, spec.label(), "clips nested too deeply");
            return;
        }
        if spec.cunit != spec.unit {
            self.report.add(Fate::Dropped, &tname, spec.label(), "a clip of clips timed in other units than its lane");
            return;
        }
        if spec.loop_.is_some() {
            self.report.add(Fate::Changed, &tname, spec.label(), "a looped clip of clips: plays once");
        }
        let hi = match spec.play_stop {
            Some(stop) => spec.vis_end.min(spec.abs_start + (stop - spec.play_start)),
            None => spec.vis_end,
        };
        let frame = Frame { unit: spec.unit, offset: spec.abs_start - spec.play_start, lo: spec.vis_start, hi };
        for c in clips.children_named("Clip") {
            self.clip(c, t, frame, out, depth + 1);
        }
    }

    /// The content segments a clip plays, in content units: `(from, to, at)` where `at` is the
    /// offset from the clip's visible start (content units), unrolling loops.
    fn segments(spec: &ClipSpec, len: f64) -> Vec<(f64, f64, f64)> {
        let head = spec.vis_start - spec.abs_start; // in timeline units (== content units when they match)
        let mut out = Vec::new();
        let stop = spec.play_stop.unwrap_or(f64::INFINITY);
        match spec.loop_ {
            None => {
                let from = spec.play_start + head;
                let to = (spec.play_start + head + len).min(stop.max(spec.play_start));
                if to > from {
                    out.push((from, to, 0.0));
                }
            }
            Some((ls, le)) => {
                // Walk the looped content from the clip start, then skip the hidden head.
                let total = head + len;
                let mut pos = spec.play_start;
                let mut at = 0.0;
                let mut guard = 0usize;
                while at < total && guard < MAX_LOOP_COPIES {
                    guard += 1;
                    let seg_end = if pos < le { le } else { pos + (total - at) };
                    let take = (seg_end - pos).min(total - at);
                    if take <= 0.0 {
                        break;
                    }
                    let (a0, a1) = (at, at + take);
                    if a1 > head {
                        let cut = (head - a0).max(0.0);
                        out.push((pos + cut, pos + take, a0 + cut - head));
                    }
                    at += take;
                    pos = ls;
                }
            }
        }
        out
    }

    fn audio_clip(&mut self, spec: &ClipSpec, content: &'p Element, points: &[&Element], t: TrackId, out: &mut Vec<Clip>) {
        let tname = self.track_name(t);
        let (audio, warps) =
            if content.name == "Warps" { (content.children.iter().find(|e| e.name == "Audio"), Some(content)) } else { (Some(content), None) };
        let Some(audio) = audio else {
            self.report.add(Fate::Dropped, &tname, spec.label(), "warped content that is not audio");
            return;
        };
        let Some(file) = audio.child("File").and_then(|f| f.attr("path").map(|p| (p, f.flag("external").unwrap_or(false)))) else {
            self.report.add(Fate::Dropped, &tname, spec.label(), "no audio file");
            return;
        };
        let src = match self.sources.get(&file_key(file.0, file.1)) {
            Some(Ok(s)) => *s,
            Some(Err(e)) => {
                let e = e.clone();
                self.report.add(Fate::Dropped, &tname, spec.label(), format!("its audio could not be read: {e}"));
                return;
            }
            None => return,
        };
        let frames = self.s.source(src).map_or(0, |x| i64::try_from(x.frames).unwrap_or(0));
        let stem = self.s.source(src).map(|x| x.name.clone()).unwrap_or_default();
        let sr = self.sr();
        let start_s = self.samples(spec.vis_start, spec.unit);
        let end_s = self.samples(spec.vis_end, spec.unit);
        // Content time → source seconds.
        let warp_map: Option<Vec<(f64, f64)>> = warps.map(|w| {
            let wunit = unit_of(w, "contentTimeUnit").unwrap_or(Unit::Seconds);
            let mut v: Vec<(f64, f64)> = w
                .children_named("Warp")
                .filter_map(|p| Some((p.num("time")?, p.num("contentTime")?)))
                .filter(|(a, b)| a.is_finite() && b.is_finite())
                .map(|(a, b)| (a, if wunit == Unit::Beats { b * 60.0 / self.s.tempo.tempo_at_tick(0) } else { b }))
                .take(MAX_EVENTS)
                .collect();
            v.sort_by(|a, b| a.0.total_cmp(&b.0));
            v
        });
        let to_source_secs = |c: f64| -> f64 {
            match &warp_map {
                Some(w) if w.len() >= 2 => interp(w, c),
                Some(w) if w.len() == 1 => w.first().map_or(c, |p| p.1 + (c - p.0)),
                _ => c,
            }
        };
        // Is the clip at the material's own speed?
        let mut stretched = false;
        if let Some(w) = &warp_map {
            for pair in w.windows(2) {
                let (Some(a), Some(b)) = (pair.first(), pair.get(1)) else { continue };
                let ta = self.content_to_samples(spec, a.0);
                let tb = self.content_to_samples(spec, b.0);
                let timeline = (tb - ta) / sr;
                let source = b.1 - a.1;
                if (timeline - source).abs() > 0.002_f64.max(source.abs() * 0.0005) {
                    stretched = true;
                }
            }
        } else if spec.cunit == Unit::Beats {
            // Content timed in beats with no warps: the file plays at the tempo's speed.
            stretched = false;
        }
        if stretched {
            self.report.add(Fate::Changed, &tname, spec.label(), "time-stretched audio: imported at its original speed");
        }
        // Segments in content units over the clip's visible length.
        let len_content = match spec.cunit {
            u if u == spec.unit => spec.vis_end - spec.vis_start,
            Unit::Seconds => (end_s - start_s) / sr,
            Unit::Beats => self.beats_at(end_s) - self.beats_at(start_s),
        };
        let segs = Self::segments_in(spec, len_content, self);
        let loop_id = (segs.len() > 1).then(|| self.s.alloc());
        let gain = clip_gain(points);
        let n = segs.len();
        for (i, (from, to, at)) in segs.into_iter().enumerate() {
            let seg_start = start_s + self.content_span_samples(spec, start_s, at);
            let seg_len = self.content_span_samples(spec, seg_start, to - from);
            let offset_secs =
                if spec.cunit == Unit::Beats && warp_map.is_none() { from * 60.0 / self.s.tempo.tempo_at_tick(0) } else { to_source_secs(from) };
            let start = clamp_pos(seg_start);
            let mut offset = clamp_pos(offset_secs * sr);
            let mut len = clamp_pos(seg_len).max(1);
            let mut start = start;
            if offset_secs < 0.0 {
                // Content before the file's start: the clip starts where the audio does.
                let skip = clamp_pos(-offset_secs * sr);
                start = start.saturating_add(skip);
                len = len.saturating_sub(skip).max(1);
                offset = 0;
            }
            if frames > 0 && offset >= frames {
                continue;
            }
            if frames > 0 {
                len = len.min(frames - offset).max(1);
            }
            let id = self.s.new_clip_id();
            let mut clip = Clip::audio(id, spec.name.clone().unwrap_or_else(|| stem.clone()), src, offset, start, len);
            clip.color = spec.color;
            clip.muted = spec.muted;
            clip.loop_id = loop_id;
            if let Some(g) = &gain {
                clip.gain_db = g.0;
                clip.gain_env =
                    g.1.iter()
                        .filter_map(|(c, db)| {
                            let at = clamp_pos(self.content_span_samples(spec, start_s, c - spec.play_start)) - (start - clamp_pos(start_s));
                            (0..=len).contains(&at).then_some((at, *db))
                        })
                        .collect();
            }
            if i == 0
                && let Some(f) = spec.fade_in
            {
                clip.fade_in = soundcraft_model::Fade {
                    len: self.fade_samples(spec, f, start as f64, false).min(len),
                    shape: soundcraft_model::FadeShape::Linear,
                    curvature: 0.0,
                };
            }
            if i + 1 == n
                && let Some(f) = spec.fade_out
            {
                let end = start.saturating_add(len) as f64;
                clip.fade_out = soundcraft_model::Fade {
                    len: self.fade_samples(spec, f, end, true).min(len),
                    shape: soundcraft_model::FadeShape::Linear,
                    curvature: 0.0,
                };
            }
            out.push(clip);
            self.audio_clips += 1;
        }
    }

    fn segments_in(spec: &ClipSpec, len: f64, _me: &Self) -> Vec<(f64, f64, f64)> {
        if spec.cunit == spec.unit {
            return Self::segments(spec, len);
        }
        // Different units: the hidden head is converted by the caller's length; no head here.
        let shifted = ClipSpec { vis_start: spec.abs_start, ..spec.clone() };
        Self::segments(&shifted, len)
    }

    /// Samples spanned by `span` content units starting at sample `from`.
    fn content_span_samples(&self, spec: &ClipSpec, from: f64, span: f64) -> f64 {
        match spec.cunit {
            Unit::Seconds => span * self.sr(),
            Unit::Beats => {
                let b = self.beats_at(from);
                self.samples(b + span, Unit::Beats) - self.samples(b, Unit::Beats)
            }
        }
    }

    /// The timeline sample where content time `c` plays (no loops).
    fn content_to_samples(&self, spec: &ClipSpec, c: f64) -> f64 {
        let start = self.samples(spec.abs_start, spec.unit);
        start + self.content_span_samples(spec, start, c - spec.play_start)
    }

    fn fade_samples(&self, spec: &ClipSpec, f: f64, at: f64, backwards: bool) -> Samples {
        match spec.fade_unit {
            Unit::Seconds => clamp_pos(f * self.sr()),
            Unit::Beats => {
                let b = self.beats_at(at);
                let other = if backwards { b - f } else { b + f };
                clamp_pos((self.samples(other, Unit::Beats) - at).abs())
            }
        }
    }

    fn midi_clip(&mut self, spec: &ClipSpec, notes: Option<&Element>, points: &[&Element], t: TrackId, out: &mut Vec<Clip>) {
        let tname = self.track_name(t);
        if !self.s.track(t).is_some_and(|tr| tr.kind.is_midi()) {
            self.report.add(Fate::Dropped, &tname, spec.label(), "notes on a track that is not a MIDI or instrument track");
            return;
        }
        let start_s = self.samples(spec.vis_start, spec.unit);
        let end_s = self.samples(spec.vis_end, spec.unit);
        let start_beats = self.beats_at(start_s);
        let len_content = match spec.cunit {
            u if u == spec.unit => spec.vis_end - spec.vis_start,
            Unit::Seconds => (end_s - start_s) / self.sr(),
            Unit::Beats => self.beats_at(end_s) - start_beats,
        };
        let segs = Self::segments_in(spec, len_content, self);
        // Content time → beats from the clip's visible start, per segment.
        let to_rel_beats = |me: &Self, c: f64, from: f64, at: f64| -> f64 {
            match spec.cunit {
                Unit::Beats => at + (c - from),
                Unit::Seconds => {
                    let s = start_s + (at + (c - from)) * me.sr();
                    me.beats_at(s) - start_beats
                }
            }
        };
        let mut seq = soundcraft_midi::Sequence::default();
        let mut dropped_events = 0usize;
        for (from, to, at) in &segs {
            if let Some(notes) = notes {
                for n in notes.children_named("Note") {
                    if seq.notes.len() >= MAX_EVENTS {
                        dropped_events += 1;
                        continue;
                    }
                    let (Some(nt), Some(nd), Some(key)) = (n.num("time"), n.num("duration"), n.int("key")) else { continue };
                    if !(nt.is_finite() && nd.is_finite()) || nt < *from || nt >= *to {
                        continue;
                    }
                    let end = (nt + nd.max(0.0)).min(*to);
                    let b0 = to_rel_beats(self, nt, *from, *at);
                    let b1 = to_rel_beats(self, end, *from, *at);
                    let start = (b0 * TPQ).round() as i64;
                    let length = (((b1 * TPQ).round() as i64) - start).max(1);
                    let vel = n.num("vel").map_or(100.0, |v| (v * 127.0).round()).clamp(1.0, 127.0) as u8;
                    let rel = n.num("rel").map_or(64.0, |v| (v * 127.0).round()).clamp(0.0, 127.0) as u8;
                    seq.notes.push(soundcraft_midi::Note {
                        pitch: key.clamp(0, 127) as u8,
                        velocity: vel,
                        release_velocity: rel,
                        channel: n.int("channel").unwrap_or(0).clamp(0, 15) as u8,
                        start,
                        length,
                    });
                }
            }
            for p in points {
                self.clip_expression(p, *from, *to, *at, &to_rel_beats, &mut seq, &tname);
            }
        }
        if dropped_events > 0 {
            self.report.add(Fate::Dropped, &tname, format!("{dropped_events} notes"), format!("more than {MAX_EVENTS} notes in one clip"));
        }
        seq.sort();
        self.notes += seq.notes.len();
        let start = clamp_pos(start_s);
        let len = clamp_pos(end_s).saturating_sub(start).max(1);
        let id = self.s.new_clip_id();
        let mut clip = Clip::midi(id, spec.name.clone().unwrap_or_else(|| tname.clone()), start, len, seq);
        clip.color = spec.color;
        clip.muted = spec.muted;
        out.push(clip);
        self.midi_clips += 1;
    }

    /// MIDI expression automation inside a clip (CC, pitch bend, pressure, program) as
    /// controller events.
    #[allow(clippy::too_many_arguments)]
    fn clip_expression(
        &mut self,
        p: &Element,
        from: f64,
        to: f64,
        at: f64,
        to_rel_beats: &dyn Fn(&Self, f64, f64, f64) -> f64,
        seq: &mut soundcraft_midi::Sequence,
        tname: &str,
    ) {
        use soundcraft_midi::CtrlKind;
        let Some(target) = p.child("Target") else { return };
        let channel = target.int("channel").unwrap_or(0).clamp(0, 15) as u8;
        let kind = match target.attr("expression") {
            Some("channelController") => CtrlKind::Cc(target.int("controller").unwrap_or(1).clamp(0, 127) as u8),
            Some("pitchBend") => CtrlKind::PitchBend,
            Some("channelPressure") => CtrlKind::ChannelPressure,
            Some("polyPressure") => CtrlKind::PolyPressure(target.int("key").unwrap_or(60).clamp(0, 127) as u8),
            Some("programChange") => CtrlKind::Program,
            Some("gain") => return,
            other => {
                self.report.add(Fate::Dropped, tname, format!("{} automation in a clip", other.unwrap_or("parameter")), "not supported inside clips");
                return;
            }
        };
        let to_value = |v: f64| -> i32 {
            let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
            match kind {
                CtrlKind::PitchBend => ((v * 16383.0).round() as i32 - 8192).clamp(-8192, 8191),
                _ => (v * 127.0).round() as i32,
            }
        };
        let pts = read_points(p);
        for (i, pt) in pts.iter().enumerate() {
            if seq.ctrls.len() >= MAX_EVENTS || pt.time < from || pt.time >= to {
                continue;
            }
            let tick = (to_rel_beats(self, pt.time, from, at) * TPQ).round() as i64;
            seq.ctrls.push(soundcraft_midi::CtrlEvent { tick, channel, kind, value: to_value(pt.value) });
            // A ramp to the next point: a value every 32nd note.
            if let Some(next) = pts.get(i + 1).filter(|n| !pt.hold && n.time < to && (n.value - pt.value).abs() > 1e-9) {
                let t1 = (to_rel_beats(self, next.time, from, at) * TPQ).round() as i64;
                let step = (soundcraft_time::TICKS_PER_QUARTER / 8).max(1);
                let mut tk = tick + step;
                while tk < t1 && seq.ctrls.len() < MAX_EVENTS {
                    let f = (tk - tick) as f64 / (t1 - tick).max(1) as f64;
                    seq.ctrls.push(soundcraft_midi::CtrlEvent { tick: tk, channel, kind, value: to_value(pt.value + (next.value - pt.value) * f) });
                    tk += step;
                }
            }
        }
    }

    // ---- automation --------------------------------------------------------------------------

    fn points(&mut self, el: &'p Element, track: Option<TrackId>, unit: Unit) {
        let Some(target) = el.child("Target") else { return };
        let tname = track.map(|t| self.track_name(t)).unwrap_or_default();
        let target = match target.attr("parameter") {
            Some(id) if Some(id) == self.tempo_id || Some(id) == self.tsig_id => return,
            Some(id) => match self.targets.get(id).cloned() {
                Some(t) => t,
                None => {
                    self.report.add(Fate::Dropped, &tname, "Automation", "of a parameter that is not in the file");
                    return;
                }
            },
            None => {
                let what = target.attr("expression").unwrap_or("expression");
                self.report.add(
                    Fate::Dropped,
                    &tname,
                    format!("{what} automation on the track"),
                    "MIDI expression automation outside clips is not supported",
                );
                return;
            }
        };
        let pts = read_points(el);
        if pts.is_empty() {
            return;
        }
        let lane_unit = el.attr("unit").map(|u| Unit2(u.to_string()));
        let abs: Vec<(Samples, f64, bool)> = pts.iter().map(|p| (clamp_pos(self.samples(p.time, unit)), p.value, p.hold)).collect();
        let unit_or = |u: &Unit2| lane_unit.clone().unwrap_or_else(|| u.clone());
        match target {
            Target::Volume(t, u) => {
                let u = unit_or(&u);
                self.write_lane(t, AutoParam::Volume, &abs, |v| db_from(v, &u.0));
            }
            Target::SendLevel(t, i, u) => {
                let u = unit_or(&u);
                self.write_lane(t, AutoParam::SendLevel(i), &abs, |v| db_from(v, &u.0));
            }
            Target::Pan(t, u) => {
                let u = unit_or(&u);
                let stereo = self.s.track(t).is_some_and(|tr| tr.format == ChannelFormat::Stereo);
                if stereo {
                    self.write_lane(t, AutoParam::Pan(0), &abs, |v| balance(pan_from(v, &u.0))[0]);
                    self.write_lane(t, AutoParam::Pan(1), &abs, |v| balance(pan_from(v, &u.0))[1]);
                } else {
                    self.write_lane(t, AutoParam::Pan(0), &abs, |v| pan_from(v, &u.0));
                }
            }
            Target::SendPan(t, i, u) => {
                let u = unit_or(&u);
                self.write_lane(t, AutoParam::SendPan(i), &abs, |v| pan_from(v, &u.0));
            }
            Target::Mute(t) => self.write_lane(t, AutoParam::Mute, &abs, |v| if v >= 0.5 { 1.0 } else { 0.0 }),
            Target::SendEnable(t, i) => self.write_lane(t, AutoParam::SendMute(i), &abs, |v| if v >= 0.5 { 0.0 } else { 1.0 }),
            Target::Unsupported(t, what) => {
                let name = self.track_name(t);
                self.report.add(Fate::Dropped, &name, format!("Automation of {what}"), "not supported here");
            }
            Target::Plugin { track, module, plugin, key, name, unit: u } => {
                let unit = unit_or(&u).0;
                let tname = self.track_name(track);
                self.pending.push(PendingAuto { track, module, plugin, key, label: format!("{tname}: {name}"), unit, points: abs });
            }
        }
    }

    fn write_lane(&mut self, t: TrackId, param: AutoParam, pts: &[(Samples, f64, bool)], conv: impl Fn(f64) -> f32) {
        let Some(tr) = self.s.track_mut(t) else { return };
        let id = param.id();
        let lane = tr.lane_mut(&param);
        lane.points.clear();
        write_points(lane, pts, conv);
        if !tr.auto_lanes.contains(&id) {
            tr.auto_lanes.push(id);
        }
        self.lanes += 1;
    }

    fn markers(&mut self, el: &Element, unit: Unit) {
        for m in el.children_named("Marker").take(10_000) {
            let Some(t) = m.num("time").filter(|v| v.is_finite()) else { continue };
            let at = clamp_pos(self.samples(t, unit));
            let name = m.attr("name").unwrap_or("Marker");
            let id = self.s.add_marker(name, soundcraft_model::MarkerKind::Marker, at, at);
            if let Some(c) = m.attr("color").and_then(parse_color)
                && let Some(mk) = self.s.markers.iter_mut().find(|x| x.id == id)
            {
                mk.color = Some(c);
            }
            if let Some(c) = m.attr("comment").filter(|c| !c.is_empty())
                && let Some(mk) = self.s.markers.iter_mut().find(|x| x.id == id)
            {
                mk.comments = c.to_string();
            }
            self.markers += 1;
        }
    }

    // ---- finish ------------------------------------------------------------------------------

    fn finish(&mut self) {
        let mut pending = std::mem::take(&mut self.pending);
        let lookup = |id: &str| crate::cmd::plugin_info(id);
        let resolved = super::resolve_pending(&mut self.s, &mut pending, &lookup);
        self.lanes += resolved;
        self.pending = pending;
        let tracks = self.s.tracks.len();
        let r = &mut self.report;
        let mut carried = vec![format!("{tracks} tracks")];
        let count = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        if self.audio_clips > 0 {
            carried.push(count(self.audio_clips, "audio clip", "audio clips"));
        }
        if self.midi_clips > 0 {
            carried.push(format!("{} ({})", count(self.midi_clips, "MIDI clip", "MIDI clips"), count(self.notes, "note", "notes")));
        }
        if self.lanes > 0 {
            carried.push(count(self.lanes, "automation lane", "automation lanes"));
        }
        if self.markers > 0 {
            carried.push(count(self.markers, "marker", "markers"));
        }
        let busses = self.s.busses.len();
        if busses > 0 {
            carried.push(count(busses, "bus", "busses"));
        }
        carried.append(&mut r.carried);
        r.carried = carried;
        // Missing media: every audio file that could not be read is named once.
        let mut missing: Vec<String> = self.sources.iter().filter_map(|(k, v)| v.as_ref().err().map(|e| format!("{k}: {e}"))).collect();
        missing.sort();
        for m in missing {
            r.add(Fate::Dropped, "", "Audio file", m);
        }
        // Tracks start with no folders open problems: a session with nothing is still valid.
        self.s.edit.selected_tracks.clear();
        self.s.sanitize();
    }
}

/// What a clip element says, worked out.
#[derive(Debug, Clone)]
struct ClipSpec {
    name: Option<String>,
    color: Option<[u8; 3]>,
    muted: bool,
    unit: Unit,
    cunit: Unit,
    abs_start: f64,
    vis_start: f64,
    vis_end: f64,
    play_start: f64,
    play_stop: Option<f64>,
    loop_: Option<(f64, f64)>,
    fade_unit: Unit,
    fade_in: Option<f64>,
    fade_out: Option<f64>,
}

impl ClipSpec {
    fn label(&self) -> String {
        match &self.name {
            Some(n) => format!("Clip '{n}'"),
            None => "Clip".into(),
        }
    }
}

fn clip_label(c: &Element) -> String {
    match c.attr("name") {
        Some(n) => format!("Clip '{n}'"),
        None => "Clip".into(),
    }
}

fn is_timeline(name: &str) -> bool {
    matches!(name, "Timeline" | "Lanes" | "Notes" | "Clips" | "ClipSlot" | "markers" | "Warps" | "Audio" | "Video" | "Points")
}

fn file_key(path: &str, external: bool) -> String {
    if external { format!("external:{path}") } else { normalise(path) }
}

fn buf_rate(info: &soundcraft_audio_io::AudioInfo) -> u32 {
    info.sample_rate
}

/// A stereo track's two panners for a balance `p` (-1..1): the far side narrows toward the near.
pub(crate) fn balance(p: f32) -> [f32; 2] {
    let p = if p.is_finite() { p.clamp(-1.0, 1.0) } else { 0.0 };
    if p >= 0.0 { [-1.0 + 2.0 * p, 1.0] } else { [-1.0, 1.0 + 2.0 * p] }
}

/// Piecewise-linear lookup in sorted `(x, y)` pairs, extrapolating the end segments.
fn interp(w: &[(f64, f64)], x: f64) -> f64 {
    let n = w.len();
    let i = w.partition_point(|p| p.0 <= x).clamp(1, n.saturating_sub(1).max(1));
    match (w.get(i - 1), w.get(i)) {
        (Some(a), Some(b)) if b.0 > a.0 => a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0),
        (Some(a), _) => a.1 + (x - a.0),
        _ => x,
    }
}

/// The points of a `Points` element, in time order.
fn read_points(el: &Element) -> Vec<Pt> {
    let mut v: Vec<Pt> = el
        .children
        .iter()
        .filter(|c| matches!(c.name.as_str(), "RealPoint" | "BoolPoint" | "IntegerPoint" | "EnumPoint" | "Point"))
        .take(MAX_POINTS)
        .filter_map(|c| {
            let time = c.num("time").filter(|t| t.is_finite())?;
            let value = match c.name.as_str() {
                "BoolPoint" => {
                    if c.flag("value")? {
                        1.0
                    } else {
                        0.0
                    }
                }
                _ => c.num("value").filter(|v| v.is_finite())?,
            };
            let hold = c.name != "RealPoint" || c.attr("interpolation") == Some("hold");
            Some(Pt { time, value, hold })
        })
        .collect();
    v.sort_by(|a, b| a.time.total_cmp(&b.time));
    v
}

/// Adds `pts` to `lane`; a held value steps (a point just before the next one keeps it).
pub(crate) fn write_points(lane: &mut soundcraft_model::AutomationLane, pts: &[(Samples, f64, bool)], conv: impl Fn(f64) -> f32) {
    for (i, (at, v, hold)) in pts.iter().enumerate() {
        let value = conv(*v);
        lane.set_point(*at, value);
        if *hold
            && let Some((next_at, nv, _)) = pts.get(i + 1)
            && *next_at > at.saturating_add(1)
            && (conv(*nv) - value).abs() > f32::EPSILON
        {
            lane.set_point(next_at - 1, value);
        }
    }
}

/// `Points` lanes aimed at parameter `id` anywhere under `el`.
fn collect_points_for<'p>(el: &'p Element, id: &str, unit: Unit, out: &mut Vec<(&'p Element, Unit)>, depth: usize) {
    if depth > 64 {
        return;
    }
    for c in &el.children {
        let unit = unit_of(c, "timeUnit").unwrap_or(unit);
        if c.name == "Points" && c.child("Target").and_then(|t| t.attr("parameter")) == Some(id) {
            out.push((c, unit));
        } else if c.name == "Lanes" {
            collect_points_for(c, id, unit, out, depth + 1);
        }
    }
}

/// Clip gain from `gain` expression lanes in a clip: the static gain (dB) and, when it varies,
/// the envelope as (content time, dB).
fn clip_gain(points: &[&Element]) -> Option<(f32, Vec<(f64, f32)>)> {
    let p = points.iter().find(|p| p.child("Target").and_then(|t| t.attr("expression")) == Some("gain"))?;
    let unit = p.attr("unit").unwrap_or("linear");
    let pts = read_points(p);
    let first = pts.first()?;
    let db = |v: f64| db_from(v, unit);
    if pts.iter().all(|x| (x.value - first.value).abs() < 1e-9) {
        return Some((db(first.value), Vec::new()));
    }
    Some((0.0, pts.iter().map(|x| (x.time, db(x.value))).collect()))
}

/// The kind of a top-level channel (no track around it).
fn channel_kind(ch: &Element, _devices: Option<&Element>) -> TrackKind {
    match ch.attr("role") {
        Some("master") => TrackKind::Master,
        Some("vca") => TrackKind::Vca,
        _ => TrackKind::Aux,
    }
}

/// The kind of track a `Track` element becomes.
fn track_kind(el: &Element, ch: Option<&Element>) -> TrackKind {
    let content: Vec<&str> = el.attr("contentType").map(|c| c.split_whitespace().collect()).unwrap_or_default();
    let role = ch.and_then(|c| c.attr("role")).unwrap_or("regular");
    let has_children = el.children.iter().any(|c| c.name == "Track");
    let has_instrument = ch.and_then(|c| c.child("Devices")).is_some_and(|d| d.children.iter().any(|x| x.attr("deviceRole") == Some("instrument")));
    if role == "master" {
        TrackKind::Master
    } else if has_children || content.contains(&"tracks") || ch.is_none() {
        TrackKind::Folder
    } else if role == "vca" {
        TrackKind::Vca
    } else if role == "effect" || role == "submix" {
        TrackKind::Aux
    } else if content.contains(&"notes") {
        if has_instrument { TrackKind::Instrument } else { TrackKind::Midi }
    } else if content.contains(&"audio") {
        TrackKind::Audio
    } else if has_instrument {
        TrackKind::Instrument
    } else {
        TrackKind::Audio
    }
}
