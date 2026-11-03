//! Tuning macros: the factory presets and the user's, saved as JSON files of [`MacroPreset`] in
//! `<preset_dir>/Vocal Tuning/<name>.json` (the folder plugin user presets use).

use crate::SoundApp;
use soundcraft_vocal::{MacroPreset, factory_presets};
use std::path::{Path, PathBuf};

/// The folder under the preset folder that holds the tuning macros.
pub const FOLDER: &str = "Vocal Tuning";
/// User preset files larger than this are ignored.
const MAX_FILE: u64 = 64 * 1024;

/// The user macro folder, when the app has a preset folder.
pub fn folder(app: &SoundApp) -> Option<PathBuf> {
    app.preset_dir.as_ref().map(|d| d.join(FOLDER))
}

/// A file name from a preset name: letters, digits, spaces and `-_.()%` kept, the rest dropped;
/// "Preset" when nothing is left.
pub fn file_stem(name: &str) -> String {
    let s: String = name.chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | '(' | ')' | '%')).take(80).collect();
    let s = s.trim().trim_matches('.').trim().to_string();
    if s.is_empty() { "Preset".into() } else { s }
}

/// The preset with every value finite and in range.
fn clamped(mut p: MacroPreset) -> MacroPreset {
    let c = |v: f32, lo: f32| if v.is_finite() { v.clamp(lo, 1.0) } else { 0.0 };
    p.centre = c(p.centre, 0.0);
    p.drift = c(p.drift, 0.0);
    p.vibrato = c(p.vibrato, -1.0);
    p.raise = c(p.raise, 0.0);
    p.lower = c(p.lower, 0.0);
    p
}

/// The user presets in `dir` (unreadable or malformed files skipped), sorted by name.
pub fn load(dir: &Path) -> Vec<MacroPreset> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<MacroPreset> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() <= MAX_FILE))
        .filter_map(|p| std::fs::read(&p).ok())
        .filter_map(|b| serde_json::from_slice::<MacroPreset>(&b).ok())
        .map(clamped)
        .filter(|p| !p.name.trim().is_empty())
        .collect();
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

/// The factory presets, then the user's sorted by name.
pub fn all(app: &SoundApp) -> Vec<MacroPreset> {
    let mut v = factory_presets();
    if let Some(dir) = folder(app) {
        v.extend(load(&dir));
    }
    v
}

/// Writes `p` to `<dir>/Vocal Tuning/<name>.json` (a temporary file renamed into place).
pub fn save(dir: &Path, p: &MacroPreset) -> std::io::Result<()> {
    let folder = dir.join(FOLDER);
    std::fs::create_dir_all(&folder)?;
    let stem = file_stem(&p.name);
    let json = serde_json::to_vec_pretty(&clamped(p.clone())).map_err(std::io::Error::other)?;
    let tmp = folder.join(format!(".{stem}.json.tmp"));
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, folder.join(format!("{stem}.json"))).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// "Centre 40 · Drift 30 · Vib −10 · Loud 25/25".
pub fn values_line(p: &MacroPreset) -> String {
    let pc = |v: f32| {
        let r = (v * 100.0).round();
        if r < 0.0 { format!("\u{2212}{:.0}", -r) } else { format!("{r:.0}") }
    };
    format!("Centre {} \u{b7} Drift {} \u{b7} Vib {} \u{b7} Loud {}/{}", pc(p.centre), pc(p.drift), pc(p.vibrato), pc(p.raise), pc(p.lower))
}
