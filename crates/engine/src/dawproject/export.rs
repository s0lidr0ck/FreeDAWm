//! Session → DAWproject. See the module docs in `mod.rs` for what maps.

use super::archive::{ArchiveWriter, normalise};
use super::state_print;
use super::xml::{Element, fmt_num};
use soundcraft_model::{
    AutoParam, AutomationLane, BusId, ChannelFormat, ClipContent, Insert, ModuleId, Route, Session, SourceId, Track, TrackId, TrackKind,
};
use soundcraft_time::Samples;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::{Seek, Write};

/// Ticks per quarter note (beat).
const TPQ: f64 = soundcraft_time::TICKS_PER_QUARTER as f64;
/// The linear gain of +12 dB, the fader's top.
const MAX_GAIN: f64 = 3.981_071_705_534_972;

/// What an export wrote, and what it could not carry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Exported {
    pub tracks: usize,
    pub clips: usize,
    pub audio_files: usize,
    pub plugin_states: usize,
    /// What other applications won't get exactly, one line each.
    pub notes: Vec<String>,
}

enum FileData {
    Bytes(Vec<u8>),
    Source(SourceId),
}

struct Ex<'s> {
    s: &'s Session,
    next: u64,
    out: Exported,
    notes: BTreeSet<String>,
    files: Vec<(String, FileData)>,
    paths: HashSet<String>,
    chan: HashMap<TrackId, String>,
    lane_id: HashMap<TrackId, String>,
    bus_chan: HashMap<BusId, String>,
    master_chan: String,
    params: HashMap<(TrackId, AutoParam), String>,
    plugin_range: HashMap<(TrackId, AutoParam), Option<(f64, f64)>>,
    sources: HashMap<SourceId, String>,
    vst3_names: Option<HashMap<String, (String, String)>>,
    tempo_id: String,
    tsig_id: String,
}

/// Writes `s` as a DAWproject archive into `w`.
pub fn export<W: Write + Seek>(s: &Session, w: W) -> Result<(W, Exported), String> {
    let mut ex = Ex {
        s,
        next: 0,
        out: Exported::default(),
        notes: BTreeSet::new(),
        files: Vec::new(),
        paths: HashSet::new(),
        chan: HashMap::new(),
        lane_id: HashMap::new(),
        bus_chan: HashMap::new(),
        master_chan: String::new(),
        params: HashMap::new(),
        plugin_range: HashMap::new(),
        sources: HashMap::new(),
        vst3_names: None,
        tempo_id: String::new(),
        tsig_id: String::new(),
    };
    let project = ex.project();
    let meta = Element::new("MetaData")
        .child_with(Element { text: s.name.clone(), ..Element::new("Title") })
        .child_with(Element { text: s.comments.clone(), ..Element::new("Comment") });
    let mut zip = ArchiveWriter::new(w);
    zip.add("project.xml", project.to_document().as_bytes())?;
    zip.add("metadata.xml", meta.to_document().as_bytes())?;
    for (path, data) in std::mem::take(&mut ex.files) {
        match data {
            FileData::Bytes(b) => {
                zip.add(&path, &b)?;
                ex.out.plugin_states += 1;
            }
            FileData::Source(id) => {
                let Some(audio) = s.pool.get(id) else { continue };
                let opts = soundcraft_audio_io::EncodeOptions {
                    format: soundcraft_audio_io::FileFormat::Wav,
                    bit_depth: soundcraft_audio_io::BitDepth::Float32,
                    dither: false,
                    bwf: None,
                };
                let bytes = soundcraft_audio_io::encode(&audio.buffer(), &opts).map_err(|e| format!("{path}: {e}"))?;
                zip.add(&path, &bytes)?;
                ex.out.audio_files += 1;
            }
        }
    }
    let w = zip.finish()?;
    ex.out.notes = ex.notes.into_iter().collect();
    Ok((w, ex.out))
}

fn bool_param(tag: &str, id: &str, name: &str, value: bool) -> Element {
    Element::new(tag).with("id", id).with("name", name).with("value", value)
}

fn real_param(tag: &str, id: &str, name: &str, unit: &str, min: f64, max: f64, value: f64) -> Element {
    Element::new(tag)
        .with("id", id)
        .with("name", name)
        .with("unit", unit)
        .with("min", fmt_num(min))
        .with("max", fmt_num(max))
        .with("value", fmt_num(value))
}

fn gain_of(db: f32) -> f64 {
    if db <= -144.0 { 0.0 } else { 10f64.powf(f64::from(db) / 20.0).min(MAX_GAIN) }
}

fn color_hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

fn strip_ids(e: &mut Element) {
    e.attrs.retain(|(k, _)| k != "id");
    for c in &mut e.children {
        strip_ids(c);
    }
}

fn safe_name(n: &str) -> String {
    let s: String = n.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' }).collect();
    let s = s.trim().to_string();
    if s.is_empty() { "file".into() } else { s.chars().take(60).collect() }
}

impl Ex<'_> {
    fn id(&mut self) -> String {
        self.next += 1;
        format!("fd{}", self.next)
    }

    fn note(&mut self, n: impl Into<String>) {
        self.notes.insert(n.into());
    }

    fn sr(&self) -> f64 {
        self.s.sample_rate.as_f64()
    }

    fn beats(&self, at: Samples) -> f64 {
        self.s.tempo.samples_to_ticks_f(at, self.s.sample_rate) / TPQ
    }

    /// A unique archive path under `dir` for a file called `name` + `ext`.
    fn path_for(&mut self, dir: &str, name: &str, ext: &str) -> String {
        let base = safe_name(name);
        let mut p = format!("{dir}/{base}.{ext}");
        let mut n = 2;
        while self.paths.contains(&p.to_ascii_lowercase()) {
            p = format!("{dir}/{base} {n}.{ext}");
            n += 1;
        }
        self.paths.insert(p.to_ascii_lowercase());
        p
    }

    fn exported(t: &Track) -> bool {
        t.kind != TrackKind::Video
    }

    fn project(&mut self) -> Element {
        let s = self.s;
        // Ids first: outputs and sends may name channels further down.
        for t in s.tracks.iter().filter(|t| Self::exported(t)) {
            let lane = self.id();
            let ch = self.id();
            self.lane_id.insert(t.id, lane);
            self.chan.insert(t.id, ch.clone());
            if matches!(t.kind, TrackKind::Aux | TrackKind::Folder)
                && let Route::Bus(b) = t.mixer.input
            {
                self.bus_chan.entry(b).or_insert(ch);
            }
        }
        let master = s.tracks.iter().find(|t| t.kind == TrackKind::Master).map(|t| t.id);
        self.master_chan = match master.and_then(|m| self.chan.get(&m)) {
            Some(c) => c.clone(),
            None => self.id(),
        };
        if s.tracks.iter().any(|t| t.kind == TrackKind::Video) {
            self.note("Video tracks are not written (DAWproject video is not exported)");
        }
        let mut structure = Element::new("Structure");
        let roots: Vec<&Track> = s.tracks.iter().filter(|t| Self::exported(t) && !self.in_folder(t)).collect();
        for t in roots {
            let el = self.track_el(t, 0);
            structure.push(el);
        }
        if master.is_none() {
            let mut ch =
                Element::new("Channel").with("id", self.master_chan.clone()).with("role", "master").with("audioChannels", 2).with("name", "Master");
            let (m, v) = (self.id(), self.id());
            ch.push(bool_param("Mute", &m, "Mute", false));
            ch.push(real_param("Volume", &v, "Volume", "linear", 0.0, MAX_GAIN, 1.0));
            let mut tr = Element::new("Track").with("id", self.id()).with("name", "Master").with("loaded", true);
            tr.push(ch);
            structure.push(tr);
        }
        let transport = self.transport();
        let arrangement = self.arrangement();
        let mut p = Element::new("Project").with("version", "1.0");
        p.push(Element::new("Application").with("name", crate::brand::APP_NAME).with("version", env!("CARGO_PKG_VERSION")));
        p.push(transport);
        p.push(structure);
        p.push(arrangement);
        p
    }

    fn in_folder(&self, t: &Track) -> bool {
        t.folder.and_then(|f| self.s.track(f)).is_some_and(|f| f.kind == TrackKind::Folder && f.id != t.id)
    }

    fn transport(&mut self) -> Element {
        let t = &self.s.tempo;
        self.tempo_id = self.id();
        self.tsig_id = self.id();
        let bpm = t.tempos().first().map_or(120.0, |e| e.bpm);
        let m = t.meters().first().copied().unwrap_or(soundcraft_time::MeterEvent { tick: 0, numerator: 4, denominator: 4 });
        Element::new("Transport").child_with(real_param("Tempo", &self.tempo_id.clone(), "Tempo", "bpm", 20.0, 999.0, bpm)).child_with(
            Element::new("TimeSignature").with("id", self.tsig_id.clone()).with("numerator", m.numerator).with("denominator", m.denominator),
        )
    }

    fn track_el(&mut self, t: &Track, depth: usize) -> Element {
        self.out.tracks += 1;
        let s = self.s;
        let content = match t.kind {
            TrackKind::Audio => Some("audio"),
            TrackKind::Instrument | TrackKind::Midi => Some("notes"),
            TrackKind::Folder => Some("tracks"),
            _ => None,
        };
        let mut el = Element::new("Track").with("id", self.lane_id.get(&t.id).cloned().unwrap_or_default());
        if let Some(c) = content {
            el.set("contentType", c);
        }
        el.set("loaded", !t.inactive);
        el.set("name", &t.name);
        el.set("color", color_hex(t.color));
        if !t.comments.is_empty() {
            el.set("comment", &t.comments);
        }
        let ch = self.channel_el(t);
        el.push(ch);
        if t.kind == TrackKind::Folder && depth < 32 {
            let kids: Vec<&Track> = s.tracks.iter().filter(|c| Self::exported(c) && c.folder == Some(t.id) && c.id != t.id).collect();
            for c in kids {
                let k = self.track_el(c, depth + 1);
                el.push(k);
            }
        }
        el
    }

    fn channel_el(&mut self, t: &Track) -> Element {
        let s = self.s;
        let feeds_sends = |b: BusId| s.tracks.iter().any(|x| x.mixer.sends.iter().flatten().any(|sl| sl.target == Route::Bus(b)));
        let role = match t.kind {
            TrackKind::Master => "master",
            TrackKind::Vca => "vca",
            TrackKind::Folder => "submix",
            TrackKind::Aux => match t.mixer.input {
                Route::Bus(b) if feeds_sends(b) => "effect",
                _ => "submix",
            },
            _ => "regular",
        };
        if t.kind == TrackKind::Vca {
            self.note("VCA tracks are written as VCA channels; which tracks they control is not carried");
        }
        let id = self.chan.get(&t.id).cloned().unwrap_or_default();
        let mut ch = Element::new("Channel").with("id", id).with("role", role).with("audioChannels", t.format.channels().max(1));
        ch.set("solo", t.mixer.solo);
        if t.kind != TrackKind::Master && t.kind != TrackKind::Vca {
            match &t.mixer.output {
                Route::Main => ch.set("destination", self.master_chan.clone()),
                Route::Bus(b) => match self.bus_chan.get(b) {
                    Some(c) => ch.set("destination", c.clone()),
                    None => {
                        let n = format!("{}: its output bus feeds no track, so it has no destination", t.name);
                        self.note(n);
                    }
                },
                Route::Hardware(h) => {
                    let n = format!("{}: output to hardware `{h}` is written as the master", t.name);
                    self.note(n);
                    ch.set("destination", self.master_chan.clone());
                }
                _ => {}
            }
        }
        // Devices.
        let mut devices = Element::new("Devices");
        if let Some(inst) = &t.instrument
            && let Some(d) = self.device_el(t, inst, "instrument")
        {
            devices.push(d);
        }
        for ins in &t.mixer.inserts {
            if let Some(d) = self.device_el(t, ins, "audioFX") {
                devices.push(d);
            }
        }
        if !devices.children.is_empty() {
            ch.push(devices);
        }
        let m = self.id();
        self.params.insert((t.id, AutoParam::Mute), m.clone());
        ch.push(bool_param("Mute", &m, "Mute", t.mixer.mute));
        let p = self.id();
        self.params.insert((t.id, AutoParam::Pan(0)), p.clone());
        let pan = match t.mixer.pan.as_slice() {
            [l, r] => (l + r) / 2.0,
            [p] => *p,
            _ => 0.0,
        };
        ch.push(real_param("Pan", &p, "Pan", "normalized", 0.0, 1.0, f64::from((pan + 1.0) / 2.0)));
        let mut sends = Element::new("Sends");
        for (i, sl) in t.mixer.sends.iter().enumerate() {
            let Some(sl) = sl else { continue };
            let Route::Bus(b) = sl.target else { continue };
            let Some(dest) = self.bus_chan.get(&b).cloned() else {
                let n = format!("{}: send {} goes to a bus no track reads, left out", t.name, i + 1);
                self.note(n);
                continue;
            };
            let slot = u8::try_from(i).unwrap_or(0);
            let (sid, en, pa, vo) = (self.id(), self.id(), self.id(), self.id());
            self.params.insert((t.id, AutoParam::SendMute(slot)), en.clone());
            self.params.insert((t.id, AutoParam::SendPan(slot)), pa.clone());
            self.params.insert((t.id, AutoParam::SendLevel(slot)), vo.clone());
            let mut se = Element::new("Send").with("id", sid).with("destination", dest).with("type", if sl.pre_fader { "pre" } else { "post" });
            se.push(bool_param("Enable", &en, "Enable", !sl.mute));
            se.push(real_param("Pan", &pa, "Pan", "normalized", 0.0, 1.0, f64::from((sl.pan.clamp(-1.0, 1.0) + 1.0) / 2.0)));
            se.push(real_param("Volume", &vo, "Volume", "linear", 0.0, MAX_GAIN, gain_of(sl.level_db)));
            sends.push(se);
        }
        if !sends.children.is_empty() {
            ch.push(sends);
        }
        let v = self.id();
        self.params.insert((t.id, AutoParam::Volume), v.clone());
        ch.push(real_param("Volume", &v, "Volume", "linear", 0.0, MAX_GAIN, gain_of(t.mixer.volume_db)));
        if t.mixer.trim_db != 0.0 {
            let n = format!("{}: input trim ({:+.1} dB) is not carried", t.name, t.mixer.trim_db);
            self.note(n);
        }
        if t.mixer.phase_invert {
            let n = format!("{}: phase invert is not carried", t.name);
            self.note(n);
        }
        ch
    }

    /// The plugin parameters of module `m` on `t` that have automation lanes.
    fn automated(t: &Track, m: ModuleId) -> Vec<String> {
        t.automation
            .iter()
            .filter_map(|l| match &l.param {
                AutoParam::Plugin { module, param } if *module == m && !l.points.is_empty() => Some(param.clone()),
                _ => None,
            })
            .collect()
    }

    fn vst3_meta(&mut self, id: &str) -> Option<(String, String)> {
        if self.vst3_names.is_none() {
            let map =
                soundcraft_vst3_host::cached().unwrap_or_default().into_iter().map(|d| (d.id.to_ascii_uppercase(), (d.name, d.vendor))).collect();
            self.vst3_names = Some(map);
        }
        self.vst3_names.as_ref().and_then(|m| m.get(&id.to_ascii_uppercase()).cloned())
    }

    fn device_el(&mut self, t: &Track, ins: &Insert, role: &str) -> Option<Element> {
        let automated = if role == "instrument" { Vec::new() } else { Self::automated(t, ins.id) };
        let third_party = soundcraft_mix::is_third_party(&ins.plugin);
        // A device that came in from a DAWproject file and is unchanged goes back as it was.
        if let Some(f) = &ins.foreign
            && automated.is_empty()
            && (!third_party || state_print(ins.state.as_deref()) == f.state_print)
            && let Ok(mut el) = Element::parse(&f.xml)
        {
            strip_ids(&mut el);
            if let Some(en) = el.child_mut("Enabled") {
                en.set("value", !ins.bypass);
            }
            if let Some(b) = f.state.as_deref().and_then(soundcraft_model::b64::decode) {
                let want =
                    el.child("State").and_then(|s| s.attr("path")).map(normalise).unwrap_or_else(|| format!("plugins/{}.bin", safe_name(&f.name)));
                let path = if self.paths.insert(want.to_ascii_lowercase()) {
                    want
                } else {
                    let ext = std::path::Path::new(&want).extension().and_then(|x| x.to_str()).unwrap_or("bin").to_string();
                    self.path_for("plugins", &f.name, &ext)
                };
                match el.child_mut("State") {
                    Some(st) => {
                        st.set("path", &path);
                        st.attrs.retain(|(k, _)| k != "external");
                    }
                    None => el.push(Element::new("State").with("path", &path)),
                }
                self.files.push((path, FileData::Bytes(b)));
            }
            el.set("id", self.id());
            return Some(el);
        }
        let info = crate::cmd::plugin_info(&ins.plugin);
        let mut params = Element::new("Parameters");
        for key in &automated {
            let pid = self.id();
            let range = info.and_then(|i| i.params.iter().find(|q| q.id == key.as_str())).map(|q| (f64::from(q.min), f64::from(q.max)));
            let value = ins.params.get(key).copied().map_or(0.0, f64::from);
            if third_party && range.is_none_or(|(lo, hi)| hi <= lo) {
                // Hosted plugins' automation is normalized in DAWproject: without the range it can't be.
                let n = format!("{}: automation of `{key}` is left out (the plugin's parameter range is not known: load it first)", t.name);
                self.note(n);
                continue;
            }
            let mut rp = match range {
                Some((lo, hi)) if hi > lo => {
                    real_param("RealParameter", &pid, key, "normalized", 0.0, 1.0, ((value - lo) / (hi - lo)).clamp(0.0, 1.0))
                }
                Some((lo, hi)) => real_param("RealParameter", &pid, key, "linear", lo, hi, value),
                None => {
                    let n = format!("{}: automation of `{key}` is written in the plugin's own values (its range is not known)", t.name);
                    self.note(n);
                    Element::new("RealParameter").with("id", pid.clone()).with("name", key).with("unit", "linear").with("value", fmt_num(value))
                }
            };
            if third_party && let Ok(n) = key.parse::<i64>() {
                rp.set("parameterID", n);
            }
            let param = AutoParam::Plugin { module: ins.id, param: key.clone() };
            self.params.insert((t.id, param.clone()), pid);
            self.plugin_range.insert((t.id, param), range.filter(|(lo, hi)| hi > lo));
            params.push(rp);
        }
        let en = self.id();
        let enabled = bool_param("Enabled", &en, "On/Off", !ins.bypass);
        let (tag, device_id, name, vendor, state): (&str, String, String, Option<String>, Option<(Vec<u8>, &str)>) = if let Some(hex) =
            ins.plugin.strip_prefix("vst3:")
        {
            let text = if cfg!(windows) {
                soundcraft_vst3_host::vstpreset::other_class_form(hex).unwrap_or_else(|| hex.to_string())
            } else {
                hex.to_string()
            };
            let meta = self.vst3_meta(&ins.plugin);
            let name = info
                .map(|i| i.name.to_string())
                .or_else(|| meta.as_ref().map(|m| m.0.clone()))
                .or_else(|| ins.foreign.as_ref().map(|f| f.name.clone()))
                .unwrap_or_else(|| "VST3 plugin".into());
            let state = ins
                .state_bytes()
                .and_then(|b| soundcraft_vst3_host::vstpreset::unpack_state(&b))
                .and_then(|(c, k)| soundcraft_vst3_host::vstpreset::write_vstpreset(&text, &c, &k))
                .map(|b| (b, "vstpreset"));
            ("Vst3Plugin", text, name, meta.map(|m| m.1).filter(|v| !v.is_empty()), state)
        } else if let Some(cid) = ins.plugin.strip_prefix("clap:") {
            let name = info.map(|i| i.name.to_string()).or_else(|| ins.foreign.as_ref().map(|f| f.name.clone())).unwrap_or_else(|| cid.to_string());
            ("ClapPlugin", cid.to_string(), name, None, ins.state_bytes().map(|b| (b, "clap-preset")))
        } else if ins.plugin.starts_with("au:") {
            let n = format!("{}: Audio Unit `{}` is written by name only (no state)", t.name, ins.plugin);
            self.note(n);
            let name = info.map(|i| i.name.to_string()).unwrap_or_else(|| ins.plugin.clone());
            ("AuPlugin", ins.plugin.clone(), name, None, None)
        } else {
            let name =
                info.map(|i| i.name.to_string()).or_else(|| ins.foreign.as_ref().map(|f| f.name.clone())).unwrap_or_else(|| ins.plugin.clone());
            self.note("FreeDAWm's own modules are written as FreeDAWm devices: other applications show them as missing");
            let mut own = ins.clone();
            own.id = ModuleId::default();
            own.foreign = None;
            let json = serde_json::to_vec(&own).ok().map(|b| (b, "json"));
            (
                "BuiltinDevice",
                format!("freedawm:{}", ins.plugin),
                format!("{} {name}", crate::brand::APP_NAME),
                Some(crate::brand::APP_NAME.to_string()),
                json,
            )
        };
        let mut el = Element::new(tag).with("id", self.id()).with("deviceID", device_id).with("deviceName", &name).with("deviceRole", role);
        if let Some(v) = vendor {
            el.set("deviceVendor", v);
        }
        el.set("loaded", ins.active);
        if !params.children.is_empty() {
            el.push(params);
        }
        el.push(enabled);
        if let Some((bytes, ext)) = state {
            let path = self.path_for("plugins", &name, ext);
            el.push(Element::new("State").with("path", &path));
            self.files.push((path, FileData::Bytes(bytes)));
        }
        Some(el)
    }

    // ---- arrangement -------------------------------------------------------------------------

    fn arrangement(&mut self) -> Element {
        let s = self.s;
        let mut lanes = Element::new("Lanes").with("id", self.id()).with("timeUnit", "beats");
        for t in s.tracks.iter().filter(|t| Self::exported(t)) {
            let mut tl = Element::new("Lanes").with("id", self.id()).with("track", self.lane_id.get(&t.id).cloned().unwrap_or_default());
            if t.kind.has_playlist() {
                let order: Vec<usize> =
                    std::iter::once(t.active_playlist).chain((0..t.playlists.len()).filter(|i| *i != t.active_playlist)).collect();
                for i in order {
                    let Some(pl) = t.playlists.get(i) else { continue };
                    if pl.clips.is_empty() && i != t.active_playlist {
                        continue;
                    }
                    let mut clips = Element::new("Clips").with("id", self.id());
                    for c in &pl.clips {
                        if let Some(el) = self.clip_el(t, c) {
                            clips.push(el);
                        }
                    }
                    tl.push(clips);
                }
            }
            for lane in &t.automation {
                if let Some(p) = self.points_el(t, lane) {
                    tl.push(p);
                }
            }
            if let Some(p) = self.stereo_pan_el(t) {
                tl.push(p);
            }
            if !tl.children.is_empty() {
                lanes.push(tl);
            }
        }
        let mut arr = Element::new("Arrangement").with("id", self.id());
        arr.push(lanes);
        let markers: Vec<_> = s.markers.iter().filter(|m| m.track.is_none()).collect();
        if !markers.is_empty() {
            let mut me = Element::new("Markers").with("id", self.id());
            for m in markers {
                let mut mk = Element::new("Marker").with("time", fmt_num(self.beats(m.start))).with("name", &m.name);
                if let Some(c) = m.color {
                    mk.set("color", color_hex(c));
                }
                if !m.comments.is_empty() {
                    mk.set("comment", &m.comments);
                }
                me.push(mk);
            }
            arr.push(me);
        }
        let tempo = &s.tempo;
        if tempo.tempos().len() > 1 || tempo.tempos().iter().any(|e| e.ramp) {
            let mut ta = Element::new("TempoAutomation").with("id", self.id()).with("unit", "bpm");
            ta.push(Element::new("Target").with("parameter", self.tempo_id.clone()));
            for e in tempo.tempos() {
                ta.push(
                    Element::new("RealPoint")
                        .with("time", fmt_num(e.tick as f64 / TPQ))
                        .with("value", fmt_num(e.bpm))
                        .with("interpolation", if e.ramp { "linear" } else { "hold" }),
                );
            }
            arr.push(ta);
        }
        if tempo.meters().len() > 1 {
            let mut ts = Element::new("TimeSignatureAutomation").with("id", self.id());
            ts.push(Element::new("Target").with("parameter", self.tsig_id.clone()));
            for m in tempo.meters() {
                ts.push(
                    Element::new("TimeSignaturePoint")
                        .with("time", fmt_num(m.tick as f64 / TPQ))
                        .with("numerator", m.numerator)
                        .with("denominator", m.denominator),
                );
            }
            arr.push(ts);
        }
        if tempo.origin() != 0 {
            self.note("The song start (bar 1 later than the session start) is not carried: times are counted from bar 1");
        }
        arr
    }

    fn clip_el(&mut self, t: &Track, c: &soundcraft_model::Clip) -> Option<Element> {
        let start = self.beats(c.start);
        let end = self.beats(c.end());
        let mut el = Element::new("Clip").with("time", fmt_num(start)).with("duration", fmt_num(end - start)).with("name", &c.name);
        if let Some(col) = c.color {
            el.set("color", color_hex(col));
        }
        if c.muted {
            el.set("enable", false);
        }
        let sr = self.sr();
        match &c.content {
            ClipContent::Audio { source, offset } => {
                let src = self.s.source(*source)?;
                if self.s.pool.get(*source).is_none() {
                    let n = format!("{}: clip `{}` has no audio loaded (missing media), left out", t.name, c.name);
                    self.note(n);
                    return None;
                }
                let path = match self.sources.get(source) {
                    Some(p) => p.clone(),
                    None => {
                        let p = self.path_for("audio", &src.name, "wav");
                        self.sources.insert(*source, p.clone());
                        self.files.push((p.clone(), FileData::Source(*source)));
                        p
                    }
                };
                el.set("contentTimeUnit", "seconds");
                el.set("playStart", fmt_num(*offset as f64 / sr));
                el.set("playStop", fmt_num((*offset + c.length) as f64 / sr));
                if c.fade_in.len > 0 || c.fade_out.len > 0 {
                    el.set("fadeTimeUnit", "seconds");
                    el.set("fadeInTime", fmt_num(c.fade_in.len as f64 / sr));
                    el.set("fadeOutTime", fmt_num(c.fade_out.len as f64 / sr));
                    if c.fade_in.shape != soundcraft_model::FadeShape::Linear || c.fade_out.shape != soundcraft_model::FadeShape::Linear {
                        self.note("Fade shapes are not carried (DAWproject stores fade lengths only)");
                    }
                }
                let audio = Element::new("Audio")
                    .with("id", self.id())
                    .with("timeUnit", "seconds")
                    .with("duration", fmt_num(src.frames as f64 / sr))
                    .with("channels", src.channels.max(1))
                    .with("sampleRate", self.s.sample_rate.hz())
                    .child_with(Element::new("File").with("path", &path));
                if c.stretch != 1.0 {
                    let n = format!("{}: clip `{}` is time-stretched, written at its original speed", t.name, c.name);
                    self.note(n);
                }
                if c.clean_alt().is_some() || c.tune_live().is_some() {
                    self.note("Clean Vocal and vocal tuning are not carried: the clips' original audio is written");
                }
                if c.gain_db != 0.0 || !c.gain_env.is_empty() {
                    let mut pts = Element::new("Points").with("id", self.id()).with("unit", "linear");
                    pts.push(Element::new("Target").with("expression", "gain"));
                    let ps = *offset as f64 / sr;
                    if c.gain_env.is_empty() {
                        pts.push(
                            Element::new("RealPoint")
                                .with("time", fmt_num(ps))
                                .with("value", fmt_num(gain_of(c.gain_db)))
                                .with("interpolation", "linear"),
                        );
                    } else {
                        for (at, db) in &c.gain_env {
                            pts.push(
                                Element::new("RealPoint")
                                    .with("time", fmt_num(ps + *at as f64 / sr))
                                    .with("value", fmt_num(gain_of(*db + c.gain_db)))
                                    .with("interpolation", "linear"),
                            );
                        }
                    }
                    el.push(Element::new("Lanes").with("id", self.id()).child_with(audio).child_with(pts));
                    self.note("Clip gain is written as a gain lane inside the clip (other applications may ignore it)");
                } else {
                    el.push(audio);
                }
            }
            ClipContent::Midi { sequence } => {
                el.set("contentTimeUnit", "beats");
                el.set("playStart", "0.0");
                el.set("playStop", fmt_num(end - start));
                let mut notes = Element::new("Notes").with("id", self.id());
                for n in &sequence.notes {
                    notes.push(
                        Element::new("Note")
                            .with("time", fmt_num(n.start as f64 / TPQ))
                            .with("duration", fmt_num(n.length as f64 / TPQ))
                            .with("channel", n.channel)
                            .with("key", n.pitch)
                            .with("vel", fmt_num(f64::from(n.velocity) / 127.0))
                            .with("rel", fmt_num(f64::from(n.release_velocity) / 127.0)),
                    );
                }
                let lanes = self.ctrl_lanes(sequence);
                if lanes.is_empty() {
                    el.push(notes);
                } else {
                    let mut l = Element::new("Lanes").with("id", self.id()).child_with(notes);
                    for p in lanes {
                        l.push(p);
                    }
                    el.push(l);
                }
            }
            ClipContent::Video { .. } => return None,
        }
        self.out.clips += 1;
        Some(el)
    }

    fn ctrl_lanes(&mut self, seq: &soundcraft_midi::Sequence) -> Vec<Element> {
        use soundcraft_midi::CtrlKind;
        let mut keys: Vec<(u8, (u8, u8))> = Vec::new();
        let key = |k: CtrlKind| match k {
            CtrlKind::Cc(n) => (0, n),
            CtrlKind::PitchBend => (1, 0),
            CtrlKind::ChannelPressure => (2, 0),
            CtrlKind::PolyPressure(p) => (3, p),
            CtrlKind::Program => (4, 0),
        };
        for c in &seq.ctrls {
            let k = (c.channel, key(c.kind));
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        let mut out = Vec::new();
        for (channel, (kind, n)) in keys {
            let mut target = Element::new("Target").with("channel", channel);
            match kind {
                0 => target = target.with("expression", "channelController").with("controller", n),
                1 => target = target.with("expression", "pitchBend"),
                2 => target = target.with("expression", "channelPressure"),
                3 => target = target.with("expression", "polyPressure").with("key", n),
                _ => target = target.with("expression", "programChange"),
            }
            let mut p = Element::new("Points").with("id", self.id()).with("unit", "normalized");
            p.push(target);
            for c in seq.ctrls.iter().filter(|c| c.channel == channel && key(c.kind) == (kind, n)) {
                let v = match c.kind {
                    CtrlKind::PitchBend => f64::from(c.value.clamp(-8192, 8191) + 8192) / 16383.0,
                    _ => f64::from(c.value.clamp(0, 127)) / 127.0,
                };
                p.push(Element::new("RealPoint").with("time", fmt_num(c.tick as f64 / TPQ)).with("value", fmt_num(v)).with("interpolation", "hold"));
            }
            out.push(p);
        }
        out
    }

    fn points_el(&mut self, t: &Track, lane: &AutomationLane) -> Option<Element> {
        if lane.points.is_empty() {
            return None;
        }
        let stereo = t.format == ChannelFormat::Stereo;
        let (key, unit, conv): (AutoParam, &str, Box<dyn Fn(f32) -> f64>) = match &lane.param {
            AutoParam::Volume => (AutoParam::Volume, "linear", Box::new(gain_of)),
            AutoParam::SendLevel(i) => (AutoParam::SendLevel(*i), "linear", Box::new(gain_of)),
            AutoParam::Pan(_) if stereo => return None,
            AutoParam::Pan(i) => (AutoParam::Pan(*i), "normalized", Box::new(|v: f32| f64::from((v.clamp(-1.0, 1.0) + 1.0) / 2.0))),
            AutoParam::SendPan(i) => (AutoParam::SendPan(*i), "normalized", Box::new(|v: f32| f64::from((v.clamp(-1.0, 1.0) + 1.0) / 2.0))),
            AutoParam::Mute => (AutoParam::Mute, "", Box::new(|v: f32| if v >= 0.5 { 1.0 } else { 0.0 })),
            AutoParam::SendMute(i) => (AutoParam::SendMute(*i), "", Box::new(|v: f32| if v >= 0.5 { 0.0 } else { 1.0 })),
            AutoParam::Trim => {
                let n = format!("{}: trim automation is not carried", t.name);
                self.note(n);
                return None;
            }
            AutoParam::Plugin { .. } => {
                let range = self.plugin_range.get(&(t.id, lane.param.clone())).copied().flatten();
                match range {
                    Some((lo, hi)) => (lane.param.clone(), "normalized", Box::new(move |v: f32| ((f64::from(v) - lo) / (hi - lo)).clamp(0.0, 1.0))),
                    None => (lane.param.clone(), "linear", Box::new(f64::from)),
                }
            }
        };
        let Some(pid) = self.params.get(&(t.id, key)).cloned() else {
            if !matches!(lane.param, AutoParam::Plugin { .. }) {
                let n = format!("{}: automation of {} has no parameter to aim at, left out", t.name, lane.param.label());
                self.note(n);
            }
            return None;
        };
        let boolean = matches!(lane.param, AutoParam::Mute | AutoParam::SendMute(_));
        let mut p = Element::new("Points").with("id", self.id());
        if !unit.is_empty() {
            p.set("unit", unit);
        }
        p.push(Element::new("Target").with("parameter", pid));
        for pt in &lane.points {
            if pt.curve.abs() > soundcraft_model::CURVE_EPSILON {
                self.note("Automation curves are written as straight lines");
            }
            let time = fmt_num(self.beats(pt.at));
            if boolean {
                p.push(Element::new("BoolPoint").with("time", time).with("value", conv(pt.value) >= 0.5));
            } else {
                p.push(Element::new("RealPoint").with("time", time).with("value", fmt_num(conv(pt.value))).with("interpolation", "linear"));
            }
        }
        Some(p)
    }

    /// A stereo track's two pan lanes as one balance lane (the centre of its image).
    fn stereo_pan_el(&mut self, t: &Track) -> Option<Element> {
        if t.format != ChannelFormat::Stereo {
            return None;
        }
        let l = t.lane(&AutoParam::Pan(0)).filter(|l| !l.points.is_empty());
        let r = t.lane(&AutoParam::Pan(1)).filter(|l| !l.points.is_empty());
        if l.is_none() && r.is_none() {
            return None;
        }
        let base = (t.mixer.pan.first().copied().unwrap_or(-1.0), t.mixer.pan.get(1).copied().unwrap_or(1.0));
        let mut times: Vec<Samples> = l.iter().chain(r.iter()).flat_map(|x| x.points.iter().map(|p| p.at)).collect();
        times.sort_unstable();
        times.dedup();
        let pid = self.params.get(&(t.id, AutoParam::Pan(0))).cloned()?;
        let mut p = Element::new("Points").with("id", self.id()).with("unit", "normalized");
        p.push(Element::new("Target").with("parameter", pid));
        for at in times {
            let lv = l.map_or(base.0, |x| x.value_at(at, base.0));
            let rv = r.map_or(base.1, |x| x.value_at(at, base.1));
            let v = f64::from(((lv + rv) / 2.0).clamp(-1.0, 1.0) + 1.0) / 2.0;
            p.push(Element::new("RealPoint").with("time", fmt_num(self.beats(at))).with("value", fmt_num(v)).with("interpolation", "linear"));
        }
        Some(p)
    }
}
