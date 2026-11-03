//! Band presets: six musicians (no song). Port of the parse / serialize half of `PresetStore.cpp`, plus
//! listing and saving a folder of them (`Presets/Bands/<Name>.json` in MS; `<config dir>/Band/Bands` here).
//! Players and targets are stored by NAME. A band preset never holds the song (key, scale, chords, bars,
//! take). Files with "factory": true are never overwritten.

use std::collections::BTreeMap;
use std::path::Path;

use crate::library::json::{Var, compare_ignore_case, parse_or_void, text_from_bytes, to_string};
use crate::library::{INSTRUMENT_NAMES, MAX_FILE_BYTES, embedded};

/// One musician of a band preset (MS `BandMember` with its `MusicianState`).
#[derive(Clone, Debug, PartialEq)]
pub struct MusicianPreset {
    pub enabled: bool,
    pub mute: bool,
    /// 1..16
    pub channel: u8,
    /// instrument name, e.g. "Piano"
    pub instrument: String,
    /// names; "Session" (or "") = Session
    pub player: String,
    pub target: String,
    /// per-musician fields without "mK_" (only [`musician_preset_fields`])
    pub params: BTreeMap<String, f32>,
}

impl Default for MusicianPreset {
    fn default() -> Self {
        Self {
            enabled: false,
            mute: false,
            channel: 1,
            instrument: String::new(),
            player: String::new(),
            target: String::new(),
            params: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BandPreset {
    pub name: String,
    pub factory: bool,
    pub tags: Vec<String>,
    pub musicians: [MusicianPreset; 6],
}

/// The per-musician fields a musician preset holds.
pub fn musician_preset_fields() -> &'static [&'static str] {
    &[
        "energy",
        "swing",
        "humanize",
        "dynamics",
        "fills",
        "respect",
        "octave",
        "length",
        "velocity",
        "speed",
        "ksoctave",
        "swaminst",
        "voice",
        "art_long",
        "art_short",
        "art_pluck",
        "art_trem",
        "ample",
        "groove",
        "amplechords",
        "pluckchords",
    ]
}

fn instrument_index(n: &str) -> i32 {
    let n = n.to_lowercase();
    INSTRUMENT_NAMES.iter().position(|i| i.to_lowercase() == n).map_or(-1, |i| i as i32)
}

fn tags_var(t: &[String]) -> Var {
    Var::Arr(t.iter().map(|s| Var::Str(s.clone())).collect())
}

fn tags_from(v: &Var) -> Vec<String> {
    v.array().map(|a| a.iter().map(|s| s.to_jstring().trim().to_lowercase()).collect()).unwrap_or_default()
}

fn state_var(s: &MusicianPreset, mut o: Vec<(String, Var)>) -> Var {
    o.push(("instrument".into(), Var::Str(s.instrument.clone())));
    o.push(("player".into(), Var::Str(if s.player.is_empty() { "Session".into() } else { s.player.clone() })));
    o.push(("target".into(), Var::Str(s.target.clone())));
    let p = s.params.iter().map(|(k, v)| (k.clone(), Var::Double(f64::from(*v)))).collect();
    o.push(("params".into(), Var::Obj(p)));
    Var::Obj(o)
}

fn state_from(j: &Var, s: &mut MusicianPreset) -> Result<(), String> {
    s.instrument = j.prop_str("instrument", "");
    if instrument_index(&s.instrument) < 0 {
        return Err(format!("unknown instrument \"{}\"", s.instrument));
    }
    s.player = j.prop_str("player", "Session");
    s.target = j.prop_str("target", "");
    if let Some(p) = j.prop("params").dyn_obj() {
        for (name, value) in p {
            if musician_preset_fields().contains(&name.as_str()) {
                s.params.insert(name.clone(), value.to_double() as f32);
            }
        }
    }
    Ok(())
}

/// MS `bandToJson`. A factory preset also writes `"factory": true` (MS never writes the flag: its factory files
/// are made by hand); [`save_band_preset`] always saves a user preset, so saved files are MS's.
pub fn band_preset_json(b: &BandPreset) -> String {
    let mut o: Vec<(String, Var)> = vec![("format".into(), Var::Int(1)), ("kind".into(), Var::Str("band".into()))];
    if b.factory {
        o.push(("factory".into(), Var::Bool(true)));
    }
    o.push(("name".into(), Var::Str(b.name.clone())));
    o.push(("tags".into(), tags_var(&b.tags)));
    let ms = b
        .musicians
        .iter()
        .map(|m| {
            let mo = vec![
                ("enabled".into(), Var::Bool(m.enabled)),
                ("mute".into(), Var::Bool(m.mute)),
                ("channel".into(), Var::Int(i64::from(m.channel))),
            ];
            state_var(m, mo)
        })
        .collect();
    o.push(("musicians".into(), Var::Arr(ms)));
    to_string(&Var::Obj(o))
}

/// MS `bandFromJson`: the error is MS's text ("not a band preset", "musician 2: unknown instrument ...").
pub fn parse_band_preset(json: &str) -> Result<BandPreset, String> {
    let j = parse_or_void(json);
    if j.prop_str("kind", "") != "band" {
        return Err("not a band preset".into());
    }
    let mut b = BandPreset { name: j.prop_str("name", "").trim().to_string(), ..BandPreset::default() };
    if b.name.is_empty() {
        return Err("the preset has no name".into());
    }
    b.tags = tags_from(j.prop("tags"));
    b.factory = j.prop_bool("factory", false);
    let ms = match j.prop("musicians").array() {
        Some(ms) if ms.len() == 6 => ms,
        _ => return Err("a band preset needs six musicians".into()),
    };
    for (k, (mj, m)) in ms.iter().zip(b.musicians.iter_mut()).enumerate() {
        m.enabled = mj.prop_bool("enabled", false);
        m.mute = mj.prop_bool("mute", false);
        m.channel = mj.prop_int("channel", k as i32 + 1).clamp(1, 16) as u8;
        if let Err(e) = state_from(mj, m) {
            return Err(format!("musician {}: {e}", k + 1));
        }
    }
    Ok(b)
}

/// Factory first (when asked), then by name ignoring case (MS `sortPresets`).
fn sort_presets(v: &mut [BandPreset], factory_first: bool) {
    v.sort_by(|a, b| {
        if a.factory != b.factory {
            return if factory_first == a.factory { std::cmp::Ordering::Less } else { std::cmp::Ordering::Greater };
        }
        compare_ignore_case(&a.name, &b.name)
    });
}

/// The factory band presets (embedded `Presets/Bands`), by name.
pub fn factory_band_presets() -> Vec<BandPreset> {
    let mut r: Vec<BandPreset> = embedded::files()
        .iter()
        .filter(|(p, _)| p.starts_with("Presets/Bands/"))
        .filter_map(|(_, b)| parse_band_preset(&text_from_bytes(b)).ok())
        .collect();
    sort_presets(&mut r, true);
    r
}

/// Every band preset in `dir/*.json` (factory-flagged first, then by name), and the files skipped with why
/// (`"Junk.json: not a band preset"`). A missing folder lists nothing.
pub fn user_band_presets(dir: &Path) -> (Vec<BandPreset>, Vec<String>) {
    use std::io::Read as _;
    let (mut r, mut skipped) = (Vec::new(), Vec::new());
    let Ok(rd) = std::fs::read_dir(dir) else { return (r, skipped) };
    let mut files: Vec<(String, std::path::PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let json = p.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("json"));
            (json && p.is_file()).then(|| (e.file_name().to_string_lossy().into_owned(), p))
        })
        .collect();
    files.sort_by(|a, b| compare_ignore_case(&a.0, &b.0));
    for (name, path) in files {
        let mut bytes = Vec::new();
        let read = std::fs::File::open(&path).and_then(|f| f.take(MAX_FILE_BYTES as u64 + 1).read_to_end(&mut bytes));
        if read.is_err() {
            skipped.push(format!("{name}: could not be read"));
            continue;
        }
        if bytes.len() > MAX_FILE_BYTES {
            skipped.push(format!("{name}: too large (over 4 MB)"));
            continue;
        }
        match parse_band_preset(&text_from_bytes(&bytes)) {
            Ok(b) => r.push(b),
            Err(e) => skipped.push(format!("{name}: {e}")),
        }
    }
    sort_presets(&mut r, true);
    (r, skipped)
}

#[cfg(not(target_arch = "wasm32"))]
fn is_factory_file(f: &Path) -> bool {
    use std::io::Read as _;
    let mut b = Vec::new();
    let read = std::fs::File::open(f).and_then(|h| h.take(MAX_FILE_BYTES as u64 + 1).read_to_end(&mut b));
    f.is_file() && read.is_ok() && b.len() <= MAX_FILE_BYTES && parse_or_void(&text_from_bytes(&b)).prop_bool("factory", false)
}

/// Writes beside, then moves (MS `writeBeside`).
#[cfg(not(target_arch = "wasm32"))]
fn write_beside(dest: &Path, text: &str) -> bool {
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut tmp_name = dest.file_name().map(std::ffi::OsStr::to_os_string).unwrap_or_default();
    tmp_name.push(".tmp");
    let tmp = dest.with_file_name(tmp_name);
    if std::fs::write(&tmp, text).is_err() {
        return false;
    }
    if std::fs::rename(&tmp, dest).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    true
}

/// MS `saveBandPreset` into `dir`: refuses to overwrite a factory preset; with `replace = false`, also refuses any
/// existing file of that name. Saved as a user preset (never "factory"), written atomically.
#[cfg(not(target_arch = "wasm32"))]
pub fn save_band_preset(dir: &Path, p: &BandPreset, replace: bool) -> Result<(), String> {
    if p.name.trim().is_empty() {
        return Err("Give the band a name.".into());
    }
    let dest = dir.join(format!("{}.json", crate::library::json::create_legal_file_name(p.name.trim())));
    if is_factory_file(&dest) {
        return Err(format!("\"{}\" is a factory band: save under another name.", p.name));
    }
    if !replace && dest.is_file() {
        return Err(format!("A band called \"{}\" already exists.", p.name));
    }
    let copy = BandPreset { factory: false, ..p.clone() };
    if !write_beside(&dest, &band_preset_json(&copy)) {
        return Err(format!("Could not write {}", dest.display()));
    }
    Ok(())
}
