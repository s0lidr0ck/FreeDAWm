//! The scenario driver: a JSON scenario (`tests/scenarios/*.json`, format in `tools/band-ref/README.md`) played
//! through `BandRuntime`, exactly as `tools/band-ref/band_ref.cpp` plays it through MIDI Studio's processor: the
//! same checks, the same order of setters, the same `ppq` stepping and the same event rule. The output is the
//! ref's lines, `<block> <offset> <status> <d1> <d2>`.
#![allow(dead_code)] // each test binary uses part of it

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};
use soundcraft_band::library::LibraryData;
use soundcraft_band::library::load::{LibraryOverlay, LibrarySource, embedded_library, load_library};
use soundcraft_band::midi_types::{MidiEvent, MidiSink};
use soundcraft_band::params::host_param_range;
use soundcraft_band::runtime::{BandRuntime, BandSettings, HostTransport, Retired};
use soundcraft_band::song::build_timeline;
use soundcraft_band::song_file::{SongFileData, song_file_from_json_checked, song_from_var};

pub fn crate_dir() -> PathBuf {
    std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from).unwrap_or_default()
}

/// FNV-1a, 64-bit, as `xtask::fidelity::fnv1a64` and band_ref's `fnv1a64Hex` (the `# case` line).
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn case_line(bytes: &[u8]) -> String {
    format!("# case {:016x}", fnv1a64(bytes))
}

/// What a ref's `# case` line hashes: the scenario's bytes, then for each `Players/*.json` / `Targets/*.json` of its
/// overlay folder (sorted by `<Players|Targets>/<file name>`, byte order) a newline, that path, a
/// newline and the file's bytes (band_ref `caseBytes`, xtask `band_refs::case_bytes`). Without an overlay: the scenario's bytes alone.
pub fn case_bytes(bytes: &[u8], overlay: Option<&std::path::Path>) -> Vec<u8> {
    let mut out = bytes.to_vec();
    let Some(dir) = overlay else { return out };
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for sub in ["Players", "Targets"] {
        let Ok(rd) = std::fs::read_dir(dir.join(sub)) else { continue };
        for e in rd.filter_map(Result::ok) {
            let p = e.path();
            let json = p.extension().is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case("json"));
            if let (true, true, Some(n)) = (json, p.is_file(), e.file_name().to_str()) {
                files.push((format!("{sub}/{n}"), p));
            }
        }
    }
    files.sort();
    for (rel, p) in files {
        out.push(b'\n');
        out.extend_from_slice(rel.as_bytes());
        out.push(b'\n');
        out.extend_from_slice(&std::fs::read(&p).unwrap_or_default());
    }
    out
}

/// A scenario's overlay folder (`tests/fixtures/libraries/<name>`), if it names one.
pub fn overlay_dir(s: &Scenario) -> Option<PathBuf> {
    s.library_overlay.as_ref().map(|d| crate_dir().join("tests").join("fixtures").join("libraries").join(d))
}

/// The `# case` line a current ref of this scenario starts with.
pub fn scenario_case_line(s: &Scenario, bytes: &[u8]) -> String {
    case_line(&case_bytes(bytes, overlay_dir(s).as_deref()))
}

#[derive(Clone, Debug, Default)]
pub struct Event {
    pub at_ppq: Option<f64>,
    pub stopped_blocks: i64,
    pub param: Option<(String, f32)>,
    pub bpm: f64,
    pub seek_to: f64,
    pub meter: Option<(i32, i32)>,
    pub stop: bool,
    pub play: bool,
}

#[derive(Clone, Debug)]
pub struct Scenario {
    pub name: String,
    pub pending: bool,
    pub sr: f64,
    pub block: usize,
    pub bpm: f64,
    pub num: i32,
    pub den: i32,
    pub bars: f64,
    /// None = defaultSong(); a string = a factory song; an object = a Song
    pub song: Option<Value>,
    pub key: Option<f64>,
    pub scale: Option<f64>,
    pub take: Option<f64>,
    /// Order: serde_json (no `preserve_order`, which would unify into every workspace crate) gives keys sorted, not in
    /// file order. That is immaterial: each flag / parameter is a distinct setter, and the one order-dependent case
    /// (an instrument change re-picking players) is staged before the players, per musician.
    pub feel: Vec<(String, bool)>,
    pub players: [Option<String>; 6],
    pub targets: [Option<String>; 6],
    pub params: Vec<(String, f32)>,
    pub events: Vec<Event>,
    /// A folder of `tests/fixtures/libraries/` whose `Players/*.json` and `Targets/*.json` replace (or add to) the
    /// factory Library's files of the same name for this scenario only (band_ref: a merged temporary copy).
    pub library_overlay: Option<String>,
}

fn is_int(v: &Value) -> bool {
    v.as_f64().is_some_and(|d| d.is_finite() && d.floor() == d)
}

/// `number (k, v, lo, hi)`: absent keeps the default.
fn number(o: &Map<String, Value>, k: &str, v: &mut f64, lo: f64, hi: f64) -> Result<(), String> {
    let Some(x) = o.get(k) else { return Ok(()) };
    match x.as_f64() {
        Some(d) if d >= lo && d <= hi => {
            *v = d;
            Ok(())
        }
        _ => Err(format!("{k} must be a number in {lo}..{hi}")),
    }
}

/// `readScenario`: every setting, read and checked before anything plays (a typo is an error, never a default).
pub fn read(bytes: &[u8]) -> Result<Scenario, String> {
    let text = String::from_utf8_lossy(bytes);
    let j: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    let Some(o) = j.as_object() else { return Err("the scenario is not a JSON object".into()) };
    const KNOWN: [&str; 19] = [
        "name",
        "pending",
        "description",
        "sample_rate",
        "block",
        "bpm",
        "num",
        "den",
        "bars",
        "song",
        "key",
        "scale",
        "take",
        "feel",
        "players",
        "targets",
        "params",
        "events",
        "library_overlay",
    ];
    if let Some(k) = o.keys().find(|k| !KNOWN.contains(&k.as_str())) {
        return Err(format!("unknown key \"{k}\""));
    }
    let name = match o.get("name").and_then(Value::as_str) {
        Some(n) if !n.is_empty() => n.to_string(),
        _ => return Err("no \"name\"".into()),
    };
    let pending = match o.get("pending") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("pending must be true or false".into()),
    };
    let (mut sr, mut block, mut bpm, mut n, mut d, mut bars) = (48000.0, 512.0, 120.0, 4.0, 4.0, 8.0);
    number(o, "sample_rate", &mut sr, 8000.0, 384000.0)?;
    number(o, "block", &mut block, 1.0, 8192.0)?;
    number(o, "bpm", &mut bpm, 20.0, 999.0)?;
    number(o, "num", &mut n, 1.0, 32.0)?;
    number(o, "den", &mut d, 1.0, 32.0)?;
    number(o, "bars", &mut bars, 0.0, 4096.0)?;
    for k in ["block", "num", "den"] {
        if o.get(k).is_some_and(|v| !is_int(v)) {
            return Err(format!("{k} must be a whole number"));
        }
    }
    let song = o.get("song").cloned();
    if song.as_ref().is_some_and(|s| !s.is_string() && !s.is_object()) {
        return Err("song must be a factory song name or a Song object".into());
    }
    let mut kst = [None, None, None];
    for (i, k) in ["key", "scale", "take"].iter().enumerate() {
        if let Some(v) = o.get(*k) {
            kst[i] = Some(v.as_f64().ok_or_else(|| format!("{k} must be a number"))?);
        }
    }
    let mut feel = Vec::new();
    if let Some(f) = o.get("feel") {
        let Some(fo) = f.as_object() else { return Err("feel must be an object".into()) };
        const FLAGS: [&str; 7] = ["natural", "tension", "bass_lock", "fills_yield", "leave_space", "dynamics", "mod_wheel"];
        for (k, v) in fo {
            if !FLAGS.contains(&k.as_str()) {
                return Err(format!("unknown feel flag \"{k}\""));
            }
            let Some(b) = v.as_bool() else { return Err(format!("feel.{k} must be true or false")) };
            feel.push((k.clone(), b));
        }
    }
    let mut names: [[Option<String>; 6]; 2] = Default::default();
    for (w, k) in ["players", "targets"].iter().enumerate() {
        let Some(a) = o.get(*k) else { continue };
        let arr = match a.as_array() {
            Some(arr) if arr.len() == 6 => arr,
            _ => return Err(format!("{k} must be an array of 6 names or nulls")),
        };
        for (i, v) in arr.iter().enumerate() {
            match v {
                Value::Null => {}
                Value::String(s) if !s.is_empty() => names[w][i] = Some(s.clone()),
                _ => return Err(format!("{k}[{i}] must be a name or null")),
            }
        }
    }
    let [players, targets] = names;
    let mut params = Vec::new();
    if let Some(p) = o.get("params") {
        let Some(po) = p.as_object() else { return Err("params must be an object".into()) };
        for (k, v) in po {
            let Some(x) = v.as_f64() else { return Err(format!("params.{k} must be a number")) };
            params.push((k.clone(), x as f32));
        }
    }
    let library_overlay = match o.get("library_overlay") {
        None => None,
        Some(Value::String(d)) if is_overlay_name(d) => Some(d.clone()),
        Some(_) => return Err("library_overlay must name a folder of tests/fixtures/libraries".into()),
    };
    let mut events = Vec::new();
    if let Some(ev) = o.get("events") {
        let Some(arr) = ev.as_array() else { return Err("events must be an array".into()) };
        for (i, e) in arr.iter().enumerate() {
            let at = format!("events[{i}]");
            let Some(eo) = e.as_object() else { return Err(format!("{at} is not an object")) };
            const EKNOWN: [&str; 9] = ["at_ppq", "stopped_blocks", "param", "value", "seek_to", "bpm", "meter", "stop", "play"];
            if let Some(k) = eo.keys().find(|k| !EKNOWN.contains(&k.as_str())) {
                return Err(format!("{at}: unknown key \"{k}\""));
            }
            let mut x = Event { seek_to: -1.0, stopped_blocks: -1, ..Event::default() };
            let (has_at, has_stopped) = (eo.contains_key("at_ppq"), eo.contains_key("stopped_blocks"));
            if has_at == has_stopped {
                return Err(format!("{at} needs exactly one of at_ppq / stopped_blocks"));
            }
            if let Some(v) = eo.get("at_ppq") {
                match v.as_f64() {
                    Some(d) if d.is_finite() && d >= 0.0 => x.at_ppq = Some(d),
                    _ => return Err(format!("{at}.at_ppq must be a number >= 0")),
                }
            }
            if let Some(v) = eo.get("stopped_blocks") {
                match v.as_f64() {
                    Some(d) if is_int(v) && d >= 0.0 => x.stopped_blocks = d as i64,
                    _ => return Err(format!("{at}.stopped_blocks must be a whole number >= 0")),
                }
            }
            if let Some(p) = eo.get("param") {
                match (p.as_str(), eo.get("value").and_then(Value::as_f64)) {
                    (Some(n), Some(v)) => x.param = Some((n.to_string(), v as f32)),
                    _ => return Err(format!("{at}: param needs a name and a numeric value")),
                }
            } else if eo.contains_key("value") {
                return Err(format!("{at}: value without param"));
            }
            if let Some(v) = eo.get("bpm") {
                match v.as_f64() {
                    Some(d) if (20.0..=999.0).contains(&d) => x.bpm = d,
                    _ => return Err(format!("{at}.bpm must be 20..999")),
                }
            }
            if let Some(v) = eo.get("seek_to") {
                match v.as_f64() {
                    Some(d) if d >= 0.0 => x.seek_to = d,
                    _ => return Err(format!("{at}.seek_to must be >= 0")),
                }
            }
            if let Some(m) = eo.get("meter") {
                let ok = m.as_array().filter(|a| a.len() == 2 && a.iter().all(|v| is_int(v) && (1.0..=32.0).contains(&v.as_f64().unwrap_or(0.0))));
                let Some(a) = ok else { return Err(format!("{at}.meter must be [num, den], whole numbers 1..32")) };
                x.meter = Some((a[0].as_f64().unwrap_or(4.0) as i32, a[1].as_f64().unwrap_or(4.0) as i32));
            }
            for (k, dst) in [("stop", &mut x.stop), ("play", &mut x.play)] {
                if let Some(v) = eo.get(k) {
                    *dst = v.as_bool().ok_or_else(|| format!("{at}.{k} must be true or false"))?;
                }
            }
            events.push(x);
        }
    }
    Ok(Scenario {
        name,
        pending,
        sr,
        block: block as usize,
        bpm,
        num: n as i32,
        den: d as i32,
        bars,
        song,
        key: kst[0],
        scale: kst[1],
        take: kst[2],
        feel,
        players,
        targets,
        params,
        events,
        library_overlay,
    })
}

/// A plain folder name (letters, digits, `-`, `_`): never a path out of `tests/fixtures/libraries/`.
fn is_overlay_name(d: &str) -> bool {
    !d.is_empty() && d.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The Library a scenario plays: the factory Library, or with its `library_overlay` folder's Players / Targets files
/// replacing (or adding to) the factory files of the same name (`LibrarySource::with_user_folder`, as the user's
/// overlay folder works; band_ref copies the same files over a temporary copy of the Library).
pub fn library(s: &Scenario) -> Result<Arc<LibraryData>, String> {
    let Some(dir) = overlay_dir(s) else { return Ok(embedded_library()) };
    if !dir.join("Players").is_dir() && !dir.join("Targets").is_dir() {
        return Err(format!("library_overlay: {} has no Players or Targets folder", dir.display()));
    }
    Ok(load_library(&LibrarySource::with_user_folder(&dir), &LibraryOverlay::default()))
}

/// A factory song file by name: `<library>/Songs/Factory/<name>.song.json` (the embedded Library), read strictly
/// with the host parameter ranges (band_ref: read leniently, and refused when anything was skipped).
pub fn factory_song(name: &str) -> Result<SongFileData, String> {
    let path = format!("Songs/Factory/{name}.song.json");
    let Some((_, bytes)) = soundcraft_band::library::embedded::files().iter().find(|(p, _)| *p == path) else {
        return Err(format!("no factory song \"{name}\""));
    };
    let text = String::from_utf8_lossy(bytes);
    song_file_from_json_checked(text.trim_start_matches('\u{feff}'), host_param_range).map_err(|e| format!("{path}: {e}"))
}

/// The processor's state after band_ref's setup steps 1-6 (step 7, prepare, is `BandRuntime::new`).
pub fn settings(s: &Scenario, lib: Arc<LibraryData>) -> Result<BandSettings, String> {
    // The song (and a factory song's own Key / Scale / Take / Natural / Tension for what the scenario leaves out).
    let mut factory = None;
    let song = match &s.song {
        None => None,
        Some(Value::String(n)) => {
            let f = factory_song(n)?;
            let song = f.song.clone();
            factory = Some(f);
            Some(song)
        }
        Some(v) => Some(song_from_var(v).map_err(|e| format!("song: {e}"))?),
    };
    // 1. MidiStudioProcessor (true).
    let mut b = BandSettings::processor_default(lib);
    // 2. setSong.
    if let Some(song) = &song {
        b.song = Some(build_timeline(song));
    }
    // 3. Key / Scale / Take.
    let pick = |v: Option<f64>, from_song: Option<i32>| v.map(|x| x as f32).or(from_song.map(|x| x as f32));
    for (id, v, fs) in [
        ("key", s.key, factory.as_ref().map(|f| f.key)),
        ("scale", s.scale, factory.as_ref().map(|f| f.scale)),
        ("take", s.take, factory.as_ref().map(|f| f.take)),
    ] {
        if let Some(v) = pick(v, fs)
            && !b.set_param(id, v)
        {
            return Err(format!("no such parameter \"{id}\""));
        }
    }
    // 4. setFeel: the factory song's natural / tension, then each flag (order immaterial: see Scenario::feel).
    if let Some(f) = &factory {
        b.feel.natural = f.natural();
        b.feel.tension = f.tension_on();
    }
    for (k, on) in &s.feel {
        let flag = match k.as_str() {
            "natural" => &mut b.feel.natural,
            "tension" => &mut b.feel.tension,
            "bass_lock" => &mut b.feel.bass_lock,
            "fills_yield" => &mut b.feel.fills_yield,
            "leave_space" => &mut b.feel.leave_space,
            "dynamics" => &mut b.feel.dynamics,
            _ => &mut b.feel.mod_wheel,
        };
        *flag = *on;
    }
    // 5. Instruments first (the mK_instrument params; each change re-picks players as the processor
    //    does), then players, then targets.
    let is_instrument = |id: &str| id.len() == 13 && id.starts_with('m') && id.ends_with("_instrument");
    for (id, v) in s.params.iter().filter(|(id, _)| is_instrument(id)) {
        if !b.set_param(id, *v) {
            return Err(format!("no such parameter \"{id}\""));
        }
    }
    for (k, p) in s.players.iter().enumerate() {
        if let Some(p) = p
            && !b.set_player(k, p)
        {
            return Err(format!("musician {}: no player \"{p}\" for its instrument", k + 1));
        }
    }
    for (k, t) in s.targets.iter().enumerate() {
        if let Some(t) = t
            && !b.set_target(k, t)
        {
            return Err(format!("musician {}: no target \"{t}\"", k + 1));
        }
    }
    // 6. The rest of the parameters; the events' parameters must exist too.
    for (id, v) in s.params.iter().filter(|(id, _)| !is_instrument(id)) {
        if !b.set_param(id, *v) {
            return Err(format!("no such parameter \"{id}\""));
        }
    }
    for e in &s.events {
        if let Some((id, _)) = &e.param
            && b.param(id).is_none()
        {
            return Err(format!("events: no such parameter \"{id}\""));
        }
    }
    Ok(b)
}

/// Records each event as a ref line under the current block index.
struct Lines<'a> {
    block: usize,
    out: &'a mut Vec<String>,
}

impl MidiSink for Lines<'_> {
    fn event(&mut self, e: &MidiEvent) {
        self.out.push(format!("{} {} {} {} {}", self.block, e.offset, e.status, e.d1, e.d2));
    }
}

/// Harness.h's play head: position, tempo, meter and a stop switch.
struct Head {
    ppq: f64,
    bpm: f64,
    playing: bool,
    num: i32,
    den: i32,
}

/// band_ref `runScenario`'s loop: the event lines (without the `# case` line).
pub fn render(s: &Scenario) -> Result<Vec<String>, String> {
    render_counted(s).map(|(lines, _)| lines)
}

/// [`render`], and how many blocks were played (band_ref prints it: "<name>: <n> blocks").
pub fn render_counted(s: &Scenario) -> Result<(Vec<String>, usize), String> {
    let lib = library(s)?;
    let mut settings = settings(s, lib)?;
    let mut rt = BandRuntime::new(s.sr, settings.clone());
    let mut h = Head { ppq: 0.0, bpm: s.bpm, playing: true, num: s.num, den: s.den };
    let end_ppq = s.bars * (f64::from(s.num) * 4.0 / f64::from(s.den)); // the first meter's bars
    let mut lines = Vec::new();
    let (mut block_index, mut stopped_for, mut next) = (0usize, 0i64, 0usize);
    let play_block = |rt: &mut BandRuntime, h: &Head, block_index: &mut usize, lines: &mut Vec<String>| {
        let t = HostTransport { playing: h.playing, bpm: h.bpm, ppq: h.ppq, num: h.num, den: h.den };
        rt.process_block(&t, s.block, &mut Lines { block: *block_index, out: lines });
        *block_index += 1;
    };
    const MAX_BLOCKS: usize = 2_000_000;
    loop {
        // Due events, in file order: at_ppq when this block starts at or after it; stopped_blocks once the transport
        // has been stopped for that many blocks.
        while let Some(e) = s.events.get(next) {
            let due = match e.at_ppq {
                Some(at) => h.ppq >= at,
                None => !h.playing && stopped_for >= e.stopped_blocks,
            };
            if !due {
                break;
            }
            if let Some((id, v)) = &e.param {
                settings.set_param(id, *v);
                let mut retired = Retired::default();
                rt.apply(settings.clone(), &mut retired);
            }
            if e.bpm > 0.0 {
                h.bpm = e.bpm;
            }
            if let Some((num, den)) = e.meter {
                h.num = num;
                h.den = den;
            }
            if e.seek_to >= 0.0 {
                h.ppq = e.seek_to;
            }
            if e.stop && h.playing {
                h.playing = false;
                stopped_for = 0;
            }
            if e.play {
                h.playing = true;
            }
            next += 1;
        }
        if h.playing && h.ppq >= end_ppq {
            break;
        }
        if !h.playing && !s.events.get(next).is_some_and(|e| e.at_ppq.is_none()) {
            break; // stopped for good
        }
        if block_index >= MAX_BLOCKS {
            return Err(format!("more than {MAX_BLOCKS} blocks: the scenario never ends"));
        }
        play_block(&mut rt, &h, &mut block_index, &mut lines);
        if h.playing {
            h.ppq += s.block as f64 / s.sr * h.bpm / 60.0;
        } else {
            stopped_for += 1;
        }
    }
    h.playing = false;
    play_block(&mut rt, &h, &mut block_index, &mut lines); // then one block with the transport stopped
    Ok((lines, block_index))
}

/// MS `Scenario.h compare`: None when equal, else the first mismatch with up to three lines of context.
pub fn compare(name: &str, reference: &[String], port: &[String]) -> Option<String> {
    let block_of = |l: &str| l.split(' ').next().unwrap_or("?").to_string();
    for (i, (r, p)) in reference.iter().zip(port).enumerate() {
        if r != p {
            let mut m = format!("FAIL {name}: first mismatch at event {i}, block {}\n", block_of(r));
            for same in &reference[i.saturating_sub(3)..i] {
                m += &format!("  same {same}\n");
            }
            m += &format!("  ref  {r}\n  port {p}\n");
            return Some(m);
        }
    }
    if reference.len() != port.len() {
        let mut m = format!("FAIL {name}: {} ref events vs {} port\n", reference.len(), port.len());
        let (longer, tag) = if reference.len() > port.len() { (reference, "ref+") } else { (port, "port+") };
        let from = reference.len().min(port.len());
        for l in longer.iter().skip(from).take(3) {
            m += &format!("  {tag} {l}\n");
        }
        return Some(m);
    }
    None
}
