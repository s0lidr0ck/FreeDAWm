//! DAWproject import and export (spec `docs/superpowers/specs/2026-10-31-dawproject-import-export-design.md`).
//!
//! DAWproject is the open project-exchange format of Bitwig and PreSonus (Studio One 6.5+,
//! Bitwig 5.0.9+, Cubase/Nuendo 14+): a zip with `project.xml` (schema: github.com/bitwig/dawproject,
//! MIT), `metadata.xml` and the embedded audio and plugin states. Our own reader and writer
//! (`quick-xml` + the workspace `zip`): the `dawproject` crate pulls `zip` with its C codecs,
//! which the web build can't take.
//!
//! What maps, both ways:
//! - transport: tempo, tempo automation (linear points are ramps), time signature changes, markers;
//! - structure: tracks in order with names, colours and comments; tracks holding tracks become
//!   routing folders (their own bus); effect and submix channels become aux tracks fed by a bus of
//!   their own, which sends and outputs aimed at them use; the master;
//! - mixer: volume (linear or dB), pan (a stereo track's balance moves its two panners), mute,
//!   solo, sends (level, pan, on/off, pre/post) and outputs;
//! - clips: audio (position, length, file offset, fades, loops unrolled into copies, warped clips
//!   at their own speed when the warps don't stretch), MIDI (notes with velocity and release,
//!   CC/pitch bend/pressure/program inside clips), clip gain (`gain` expression lanes), alias
//!   clips and clips of clips (flattened); every `Clips` lane past a track's first is a playlist
//!   of its own (take lanes; the first stays active);
//! - automation: volume, pan, mute, send level/pan/on, and plugin parameters (once the plugin's
//!   parameter ranges are known: `resolve_pending`);
//! - devices: VST3 (state from the `.vstpreset`) and CLAP plugins load with their state;
//!   anything else (not installed, VST2, AU, another DAW's built-in devices, note FX) becomes an
//!   inactive placeholder slot holding the device element and its state file
//!   (`Insert::foreign`), which export writes back byte for byte. FreeDAWm's own modules export as
//!   `BuiltinDevice deviceID="freedawm:<id>"` with the module as JSON state, and come back whole.
//!
//! Embedded audio is decoded into the session's memory at import (`Source::unsaved`) and written
//! into the session's Audio Files folder on the first save, like a recording; nothing outside the
//! archive is touched. Everything not carried exactly is listed in the [`Report`].

pub mod archive;
pub mod export;
pub mod import;
pub mod report;
pub mod xml;

pub use report::{Fate, Item, Report};

use soundcraft_model::{AutoParam, ModuleId, Note, Session, TrackId};
use soundcraft_time::Samples;

/// The file extension.
pub const EXTENSION: &str = "dawproject";

/// A time unit of the format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Beats,
    Seconds,
}

/// Plugin parameter automation read from the file, waiting for the parameter's range: hosted
/// plugins' values in DAWproject are normalized (0..1, whatever the `unit` says), as are
/// FreeDAWm modules' when written `normalized`; modules take plain values.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingAuto {
    pub track: TrackId,
    pub module: ModuleId,
    pub plugin: String,
    /// The parameter's id on the module (`parameterID`).
    pub key: String,
    /// "Track: Device Parameter", for the report.
    pub label: String,
    /// The lane's unit (`normalized` needs the range).
    pub unit: String,
    /// (time, value as written, held until the next point).
    pub points: Vec<(Samples, f64, bool)>,
}

/// Writes the pending lanes whose plugin parameter range is known now (`lookup`), or that need
/// none (a FreeDAWm module's plain values); returns how many were written and keeps the rest.
pub fn resolve_pending(
    s: &mut Session,
    pending: &mut Vec<PendingAuto>,
    lookup: &dyn Fn(&str) -> Option<&'static soundcraft_dsp::PluginInfo>,
) -> usize {
    let mut done = 0;
    pending.retain(|p| {
        let normalized = p.unit == "normalized" || soundcraft_mix::is_third_party(&p.plugin);
        let range = if !normalized {
            None
        } else {
            match lookup(&p.plugin).and_then(|i| i.params.iter().find(|q| q.id == p.key)) {
                Some(q) => Some((f64::from(q.min), f64::from(q.max))),
                None => return true,
            }
        };
        let conv = |v: f64| -> f32 {
            match range {
                Some((lo, hi)) => (lo + v.clamp(0.0, 1.0) * (hi - lo)) as f32,
                None => v as f32,
            }
        };
        let Some(tr) = s.track_mut(p.track) else { return false };
        let param = AutoParam::Plugin { module: p.module, param: p.key.clone() };
        let id = param.id();
        let lane = tr.lane_mut(&param);
        lane.points.clear();
        import::write_points(lane, &p.points, conv);
        if !tr.auto_lanes.contains(&id) {
            tr.auto_lanes.push(id);
        }
        done += 1;
        false
    });
    done
}

/// The last step of an import, on the engine's thread once plugins may have been made: the
/// pending plugin automation that can be written now is, the rest is reported, and the report
/// is saved in the session's Notes tab.
pub fn finish_import(s: &mut Session, report: &mut Report, pending: &mut Vec<PendingAuto>) {
    let n = resolve_pending(s, pending, &|id| crate::cmd::plugin_info(id));
    if n > 0 {
        report.carried.push(format!("{n} plugin automation lane{}", if n == 1 { "" } else { "s" }));
    }
    for p in pending.drain(..) {
        report.add(Fate::Dropped, "", format!("Automation of {}", p.label), "the plugin's parameter ranges are not known (not loaded)");
    }
    add_report_notes(s, report);
}

/// The report as notes in the Notes tab, tagged `#dawproject` (split at the note length limit).
pub fn add_report_notes(s: &mut Session, report: &Report) {
    let text = report.to_text();
    let mut chunk = String::new();
    let mut chunks = Vec::new();
    for line in text.lines() {
        if chunk.chars().count() + line.chars().count() + 1 > soundcraft_model::MAX_NOTE_CHARS - 40 && !chunk.is_empty() {
            chunks.push(std::mem::take(&mut chunk));
        }
        if !chunk.is_empty() {
            chunk.push('\n');
        }
        chunk.push_str(line);
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    let total = chunks.len();
    for (i, c) in chunks.into_iter().take(50).enumerate() {
        let text = if total > 1 { format!("{c}\n(part {} of {total})", i + 1) } else { c };
        let id = s.alloc();
        s.notes.push(Note { id, text, done: false, tags: vec!["dawproject".into()], at: None, created: String::new() });
    }
    soundcraft_model::sanitize_notes(&mut s.notes);
}

/// dB from a volume parameter's value in `unit` (`linear` gain, `decibel`; `normalized` is read
/// as a gain too).
pub fn db_from(v: f64, unit: &str) -> f32 {
    let db = match unit {
        "decibel" => v,
        _ => {
            if v <= 0.0 {
                -144.0
            } else {
                20.0 * v.log10()
            }
        }
    };
    if db.is_finite() {
        db.clamp(-144.0, 12.0) as f32
    } else if db > 0.0 {
        12.0
    } else {
        -144.0
    }
}

/// Pan -1..1 from a pan parameter's value in `unit` (`normalized`: 0..1 with 0.5 centre;
/// `percent`: -100..100; `linear`: -1..1).
pub fn pan_from(v: f64, unit: &str) -> f32 {
    let p = match unit {
        "normalized" => v * 2.0 - 1.0,
        "percent" => v / 100.0,
        _ => v,
    };
    if p.is_finite() { p.clamp(-1.0, 1.0) as f32 } else { 0.0 }
}

/// `#RRGGBB` (or `#RRGGBBAA`) as RGB.
pub fn parse_color(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().strip_prefix('#')?;
    if !(h.len() == 6 || h.len() == 8) || !h.is_ascii() {
        return None;
    }
    let byte = |i: usize| h.get(i..i + 2).and_then(|x| u8::from_str_radix(x, 16).ok());
    Some([byte(0)?, byte(2)?, byte(4)?])
}

/// A fingerprint of a module's stored state (FNV-1a; 0 for none).
pub fn state_print(state: Option<&str>) -> u64 {
    let Some(s) = state else { return 0 };
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h.max(1)
}

/// Opens the archive at `path` (a file inside the read root) and imports it.
pub fn import_path(
    path: &str,
    installed: import::Installed,
    progress: &mut dyn FnMut(crate::open_job::OpenProgress),
) -> Result<import::Imported, String> {
    let p = std::path::Path::new(path);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Imported".into());
    let file = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string());
    let dir = p.parent().map(std::path::Path::to_path_buf);
    #[cfg(not(target_arch = "wasm32"))]
    {
        let f = crate::io::open_read(p).map_err(|e| e.to_string())?;
        let mut a = archive::Archive::new(std::io::BufReader::new(f))?;
        import::import(&mut a, &stem, &file, dir.as_deref(), installed, progress)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let bytes = crate::io::read_file(p).map_err(|e| e.to_string())?;
        import_bytes(&bytes, &stem, &file, installed, progress)
    }
}

/// Imports an archive held in memory (no folder: external files can't be found).
pub fn import_bytes(
    bytes: &[u8],
    name: &str,
    file: &str,
    installed: import::Installed,
    progress: &mut dyn FnMut(crate::open_job::OpenProgress),
) -> Result<import::Imported, String> {
    let mut a = archive::Archive::new(std::io::Cursor::new(bytes))?;
    import::import(&mut a, name, file, None, installed, progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_and_colours() {
        assert_eq!(db_from(1.0, "linear"), 0.0);
        assert!((db_from(0.5, "linear") + 6.0206).abs() < 1e-3);
        assert_eq!(db_from(0.0, "linear"), -144.0);
        assert_eq!(db_from(-3.0, "decibel"), -3.0);
        assert_eq!(db_from(f64::INFINITY, "decibel"), 12.0);
        assert_eq!(pan_from(0.5, "normalized"), 0.0);
        assert_eq!(pan_from(1.0, "normalized"), 1.0);
        assert_eq!(pan_from(-50.0, "percent"), -0.5);
        assert_eq!(parse_color("#FF8000"), Some([255, 128, 0]));
        assert_eq!(parse_color("#ff8000ff"), Some([255, 128, 0]));
        assert_eq!(parse_color("red"), None);
        assert_eq!(parse_color("#ééé"), None);
        assert_eq!(state_print(None), 0);
        assert_ne!(state_print(Some("a")), state_print(Some("b")));
    }
}
