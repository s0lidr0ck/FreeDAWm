//! The user's own files the app lists: track presets and session templates.
//!
//! Both live in the settings directory, next to the user plugin presets (`Presets/`):
//! `Track Presets/<name>.json` (written by `track.save_preset`) and `Templates/<name>.scraft`
//! (written by `session.save_template`). The desktop app sets [`UserDirs`]; the web build has
//! none, and the menus then offer only Browse….

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Track preset files' extension.
pub const TRACK_PRESET_EXT: &str = "json";

/// Where the user's track presets and templates are kept (none in the browser).
#[derive(Debug, Clone, Default)]
pub struct UserDirs {
    pub track_presets: Option<PathBuf>,
    pub templates: Option<PathBuf>,
}

impl UserDirs {
    /// Both folders under one settings directory.
    pub fn under(config: &Path) -> UserDirs {
        UserDirs { track_presets: Some(config.join("Track Presets")), templates: Some(config.join("Templates")) }
    }
}

/// One listed file: its name (the file stem), path and when it was last written.
#[derive(Debug, Clone, PartialEq)]
pub struct UserFile {
    pub name: String,
    pub path: PathBuf,
    pub modified: Option<SystemTime>,
}

/// Most files listed from one folder, the newest (a folder is user-controlled input).
const MAX_LISTED: usize = 500;
/// Most folder entries looked at to find them.
const MAX_SCANNED: usize = 20_000;

/// The files in `dir` with extension `ext` (case-insensitive), newest first, then by name. A
/// missing or unreadable folder lists nothing.
pub fn list(dir: Option<&Path>, ext: &str) -> Vec<UserFile> {
    list_any(dir, &[ext])
}

/// [`list`] for files with any of `exts`.
pub fn list_any(dir: Option<&Path>, exts: &[&str]) -> Vec<UserFile> {
    let Some(dir) = dir else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<UserFile> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .take(MAX_SCANNED)
        .filter(|p| p.is_file() && p.extension().and_then(|x| x.to_str()).is_some_and(|x| exts.iter().any(|e| x.eq_ignore_ascii_case(e))))
        .filter_map(|path| {
            let name = path.file_stem()?.to_string_lossy().into_owned();
            let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            Some(UserFile { name, path, modified })
        })
        .collect();
    // Sorted before the cut, so the newest are the ones kept.
    out.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out.truncate(MAX_LISTED);
    out
}

/// The track presets, newest first.
pub fn track_presets(dirs: &UserDirs) -> Vec<UserFile> {
    list(dirs.track_presets.as_deref(), TRACK_PRESET_EXT)
}

/// The saved templates (`.fdawm`, and `.scraft` ones saved before the rename), newest first.
pub fn templates(dirs: &UserDirs) -> Vec<UserFile> {
    list_any(dirs.templates.as_deref(), &soundcraft_model::SESSION_EXTENSIONS)
}

/// A file name for `name` in a folder: path separators and characters Windows refuses become
/// `_`, and an empty name becomes "Untitled".
pub fn file_name(name: &str) -> String {
    let clean: String = name
        .trim()
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c })
        .collect();
    let clean = clean.trim_matches('.').trim();
    if clean.is_empty() { "Untitled".into() } else { clean.chars().take(120).collect() }
}

/// What a template holds, for the New dialog's details line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TemplateInfo {
    pub tracks: usize,
    pub sample_rate: u32,
}

/// Largest template file read for its details.
const MAX_TEMPLATE_BYTES: u64 = 64 << 20;

/// The track count and sample rate of the template at `path` (`None` when it can't be read).
pub fn template_info(path: &Path) -> Option<TemplateInfo> {
    if std::fs::metadata(path).ok()?.len() > MAX_TEMPLATE_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let s = soundcraft_model::Session::from_json(&text).ok()?;
    Some(TemplateInfo { tracks: s.tracks.len(), sample_rate: s.sample_rate.hz() })
}

/// `Oct 12`: a short date (UTC) for lists.
pub fn short_date(t: SystemTime) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let days = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() / 86_400;
    let (_, m, d) = civil_from_days(days);
    let month = MONTHS.get(usize::try_from(m.saturating_sub(1)).unwrap_or(0)).copied().unwrap_or("Jan");
    format!("{month} {d:02}")
}

/// `24 tracks · 48 kHz · saved Oct 12`.
pub fn template_details(info: Option<TemplateInfo>, modified: Option<SystemTime>) -> String {
    let mut parts = Vec::new();
    if let Some(i) = info {
        parts.push(if i.tracks == 1 { "1 track".to_string() } else { format!("{} tracks", i.tracks) });
        let khz = f64::from(i.sample_rate) / 1000.0;
        parts.push(if khz.fract() == 0.0 { format!("{khz:.0} kHz") } else { format!("{khz:.1} kHz") });
    } else {
        parts.push("can't be read".into());
    }
    if let Some(t) = modified {
        parts.push(format!("saved {}", short_date(t)));
    }
    parts.join(" · ")
}

/// Days since 1970-01-01 to (year, month, day) (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days.saturating_add(719_468);
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = era.saturating_mul(400).saturating_add(yoe).saturating_add(u64::from(m <= 2));
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sc-user-files-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn lists_newest_first_and_only_its_extension() {
        let d = scratch("list");
        for (name, age) in [("old.json", 300), ("new.json", 10), ("mid.JSON", 100)] {
            let p = d.join(name);
            std::fs::write(&p, "{}").unwrap();
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(SystemTime::now() - Duration::from_secs(age)).unwrap();
        }
        std::fs::write(d.join("notes.txt"), "x").unwrap();
        std::fs::create_dir_all(d.join("folder.json")).unwrap();
        let names: Vec<String> = list(Some(&d), "json").into_iter().map(|f| f.name).collect();
        assert_eq!(names, ["new", "mid", "old"]);
        assert!(list(Some(&d.join("missing")), "json").is_empty());
        assert!(list(None, "json").is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_full_folder_keeps_the_newest() {
        let d = scratch("many");
        for k in 0..(MAX_LISTED + 20) {
            let p = d.join(format!("p{k:04}.json"));
            std::fs::write(&p, "{}").unwrap();
            // p0000 is the newest, written last in time order.
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(SystemTime::now() - Duration::from_secs(10 + k as u64)).unwrap();
        }
        let got = list(Some(&d), "json");
        assert_eq!(got.len(), MAX_LISTED);
        assert_eq!(got.first().map(|f| f.name.as_str()), Some("p0000"));
        assert_eq!(got.last().map(|f| f.name.clone()), Some(format!("p{:04}", MAX_LISTED - 1)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("Lead Vox – bright"), "Lead Vox – bright");
        assert_eq!(file_name("a/b\\c:d"), "a_b_c_d");
        assert_eq!(file_name("  "), "Untitled");
        assert_eq!(file_name(".."), "Untitled");
    }

    #[test]
    fn dates_and_details() {
        // 2026-10-12 is day 20738 since the epoch.
        let t = std::time::UNIX_EPOCH + Duration::from_secs(20_738 * 86_400 + 3600);
        assert_eq!(short_date(t), "Oct 12");
        assert_eq!(template_details(Some(TemplateInfo { tracks: 24, sample_rate: 48_000 }), Some(t)), "24 tracks · 48 kHz · saved Oct 12");
        assert_eq!(template_details(Some(TemplateInfo { tracks: 1, sample_rate: 44_100 }), None), "1 track · 44.1 kHz");
    }

    #[test]
    fn template_info_reads_a_saved_session() {
        let d = scratch("tpl");
        let mut s = soundcraft_model::Session::new("T", soundcraft_time::SampleRate::new(96_000).unwrap());
        s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, None);
        let p = d.join("t.scraft");
        std::fs::write(&p, s.to_json().unwrap()).unwrap();
        assert_eq!(template_info(&p), Some(TemplateInfo { tracks: 1, sample_rate: 96_000 }));
        std::fs::write(&p, "not json").unwrap();
        assert_eq!(template_info(&p), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
