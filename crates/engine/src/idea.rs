//! A FreeDAWm Recorder idea's sidecar (`<idea>.fdidea`, JSON, beside its files), read when one of
//! its files is imported (`file.import_midi`, `file.import_audio`: the files the Recorder's "Open
//! in FreeDAWm" names on the command line):
//!
//! - `tempo: {bpm, beats}`: the click it was recorded to. An audio file imported into a session
//!   with nothing in it yet sets the session's tempo and meter (x/4) from it (a MIDI file carries
//!   its own tempo).
//! - `keys: {"<layer>": {plugin, preset, params, modulation, volume_db}}`: the built-in
//!   instrument a layer played on keys was heard through. Its MIDI file's tracks get that
//!   instrument, those settings and that volume.
//!
//! Layer `n` of idea `X` is `X (layer n).mid` (layer 1 is `X.mid`). Anything missing or damaged
//! is left alone: the import is as it would be without the sidecar.

use crate::Engine;
use serde_json::{Value, json};
use soundcraft_model::TrackId;
use std::path::{Path, PathBuf};

/// The sidecar of the idea `path` belongs to, and the file's layer number.
fn sidecar_of(path: &Path) -> Option<(Value, u32)> {
    let stem = path.file_stem()?.to_str()?;
    let stem = stem.strip_suffix(" (keys)").unwrap_or(stem);
    let (idea, n) = match stem.strip_suffix(')').and_then(|h| h.rfind(" (layer ").map(|i| (h, i))) {
        Some((h, i)) => match h.get(i + 8..).and_then(|n| n.parse::<u32>().ok()).filter(|n| *n >= 2) {
            Some(n) => (h.get(..i)?.to_string(), n),
            None => (stem.to_string(), 1),
        },
        None => (stem.to_string(), 1),
    };
    let side: PathBuf = path.with_file_name(format!("{idea}.fdidea"));
    let bytes = std::fs::read(side).ok()?;
    Some((serde_json::from_slice(&bytes).ok()?, n))
}

/// After a MIDI file at `path` was imported onto `tracks`: the instrument its layer was heard
/// through. Returns whether one was set.
pub fn apply_to_midi(e: &mut Engine, tracks: &[TrackId], path: &Path) -> bool {
    let Some((side, n)) = sidecar_of(path) else { return false };
    let Some(k) = side.get("keys").and_then(|k| k.get(n.to_string())).filter(|k| k.is_object()) else { return false };
    let Some(plugin) = k.get("plugin").and_then(Value::as_str) else { return false };
    let mut set = false;
    for t in tracks {
        let mut p = json!({"tracks": [t.0], "plugin": plugin});
        for f in ["params", "modulation", "preset"] {
            if let Some(v) = k.get(f).filter(|v| !v.is_null()) {
                p[f] = v.clone();
            }
        }
        match e.execute("mix.instrument", &p) {
            Ok(_) => set = true,
            Err(err) => log::warn!("{}: its instrument could not be set: {err}", path.display()),
        }
        if let Some(db) = k.get("volume_db").and_then(Value::as_f64).filter(|d| d.is_finite()) {
            let _ = e.execute("mix.volume", &json!({"tracks": [t.0], "db": db.clamp(-60.0, 12.0)}));
        }
    }
    set
}

/// Before an audio file at `path` is imported into `e`: the idea's tempo, when the session has
/// nothing in it yet. Returns whether it was set.
pub fn apply_tempo(e: &mut Engine, path: &Path) -> bool {
    if e.session().tracks.iter().any(|t| !t.clips().is_empty()) {
        return false;
    }
    let Some((side, _)) = sidecar_of(path) else { return false };
    let Some(t) = side.get("tempo").filter(|t| t.is_object()) else { return false };
    let bpm = t.get("bpm").and_then(Value::as_f64).filter(|b| b.is_finite() && (20.0..=400.0).contains(b));
    let beats = t.get("beats").and_then(Value::as_u64).and_then(|b| u32::try_from(b).ok()).filter(|b| (1..=16).contains(b));
    let (Some(bpm), Some(beats)) = (bpm, beats) else { return false };
    match soundcraft_time::TempoMap::new(bpm, beats, 4) {
        Ok(map) => {
            e.session_mut().tempo = map;
            true
        }
        Err(_) => false,
    }
}
