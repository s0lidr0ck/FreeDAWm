//! The recorder's settings, in files of its own beside the DAW's (never the DAW's `network.json`):
//!
//! - `recorder.json`: the ideas folder, the audio devices, the input channel, the MIDI ports, the
//!   mix's volume ([`Prefs`]).
//! - `recorder-network.json`: the booth it follows and its pairing, in the DAW's network settings
//!   format ([`soundcraft_net::config::NetConfig`]). Its own pairing: the booth sees
//!   "<PC> Recorder" as a device of its own, beside the DAW on the same PC.
//! - `Recorder Outbox/`: takes waiting to be sent to the booth (the follower's outbox).
//!
//! The settings folder is the DAW's (`%APPDATA%\FreeDAWm` on Windows), or `$FREEDAWM_CONFIG_DIR`.

use serde::{Deserialize, Serialize};
use soundcraft_engine::brand;
use soundcraft_playback::devices::AudioConfig;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What the recorder remembers between runs (`recorder.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Where takes are saved (empty: [`default_ideas_folder`]).
    pub ideas_folder: String,
    /// The audio system and devices (input records, output plays the booth's mix and Play).
    pub audio: AudioConfig,
    /// Record audio from the input (off: MIDI only).
    pub audio_on: bool,
    /// The first input channel recorded (0-based), and whether the pair from it is recorded.
    pub channel: usize,
    pub stereo: bool,
    /// MIDI input ports by name: off when false (a port never seen records).
    pub midi: BTreeMap<String, bool>,
    /// The booth's mix in the output, at this volume (linear).
    pub listen: bool,
    pub listen_gain: f32,
    /// Start recording as soon as the Recorder opens (and its devices are ready).
    pub record_on_open: bool,
    /// Where the window was last (its top-left corner on the desktop, points).
    pub window_pos: Option<[f32; 2]>,
    /// The window's last size (points).
    pub window_size: Option<[f32; 2]>,
    /// Layers: count 3-2-1 on the screen before a layer starts (else it starts at once).
    pub count_in: bool,
    /// Layers: the manual nudge, in milliseconds (more: layers are placed earlier).
    pub layer_nudge_ms: f32,
    /// Layers: the round trip **Line up** measured, with the devices it was measured on.
    pub layer_measured: Option<Measured>,
    /// The headphones hint was shown (it shows once).
    pub layer_hint_seen: bool,
}

/// A loopback round trip measured by **Line up**.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measured {
    /// The devices, buffer and rate it holds for (`Rig::latency_key`).
    pub key: String,
    pub frames: i64,
    /// The input feed's latency it went through.
    pub feed: u32,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            ideas_folder: String::new(),
            audio: AudioConfig::default(),
            audio_on: true,
            channel: 0,
            stereo: false,
            midi: BTreeMap::new(),
            listen: false,
            listen_gain: 1.0,
            record_on_open: false,
            window_pos: None,
            window_size: None,
            count_in: true,
            layer_nudge_ms: 0.0,
            layer_measured: None,
            layer_hint_seen: false,
        }
    }
}

impl Prefs {
    /// Reads `path`; missing or unreadable, the defaults.
    pub fn load(path: &Path) -> Prefs {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Writes `path` through a temporary file, so a crash never leaves half of it.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    /// The ideas folder in use.
    pub fn ideas(&self) -> PathBuf {
        if self.ideas_folder.trim().is_empty() { default_ideas_folder() } else { PathBuf::from(self.ideas_folder.trim()) }
    }
}

/// Where the recorder keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub prefs: PathBuf,
    pub network: PathBuf,
    pub outbox: PathBuf,
}

impl Paths {
    /// In settings folder `dir`.
    pub fn in_dir(dir: &Path) -> Paths {
        Paths { prefs: dir.join("recorder.json"), network: dir.join("recorder-network.json"), outbox: dir.join("Recorder Outbox") }
    }

    /// In the DAW's settings folder (`None` when there is none: settings are then not kept).
    pub fn system() -> Option<Paths> {
        settings_dir().map(|d| Paths::in_dir(&d))
    }
}

/// The DAW's settings folder: `$FREEDAWM_CONFIG_DIR` when set (empty: none), `FreeDAWmData`
/// beside the program in portable mode (`portable.txt` there), else the profile's.
pub fn settings_dir() -> Option<PathBuf> {
    if let Some(dir) = brand::env_var_os("CONFIG_DIR") {
        return (!dir.is_empty()).then(|| PathBuf::from(dir));
    }
    let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
    if let Some(d) = exe_dir.filter(|d| d.join("portable.txt").is_file()) {
        return Some(d.join(brand::PORTABLE_DATA_DIR));
    }
    brand::profile_settings_dirs(brand::Platform::current(), |n| std::env::var_os(n)).map(|(dir, _)| dir)
}

/// `Documents/FreeDAWm Ideas` in the user's home folder (the temporary folder without one).
pub fn default_ideas_folder() -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|h| !h.is_empty());
    match home {
        Some(h) => PathBuf::from(h).join("Documents").join("FreeDAWm Ideas"),
        None => std::env::temp_dir().join("FreeDAWm Ideas"),
    }
}

/// This PC's name as a booth sees the recorder: "<computer> Recorder".
pub fn recorder_name(computer: &str) -> String {
    let c = computer.trim();
    if c.is_empty() { "Recorder".into() } else { format!("{c} Recorder") }
}

// ---------------------------------------------------------------------------------------------
// Names of takes

/// A wall-clock time, `(year, month, day, hour, minute, second)`.
pub type Stamp = (i64, u32, u32, u32, u32, u32);

/// Now, in seconds since 1970 (UTC).
pub fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
}

/// Now, in the computer's local time.
pub fn now_stamp() -> Stamp {
    local_stamp(now_secs())
}

/// `secs` (since 1970, UTC) in the computer's local time: its time zone's offset at that time as
/// the system gives it (`soundcraft_sysmem::utc_offset_secs`; on Windows the offset in force
/// now), UTC when it gives none.
pub fn local_stamp(secs: i64) -> Stamp {
    let offset = soundcraft_sysmem::utc_offset_secs(secs).unwrap_or(0);
    stamp_of(u64::try_from(secs.saturating_add(i64::from(offset))).unwrap_or(0))
}

/// The day of the week of a date, 0 for Monday … 6 for Sunday.
pub fn weekday(s: Stamp) -> usize {
    // Days since 1970-01-01 (a Thursday), Howard Hinnant's days-from-civil.
    let (y, m, d) = (s.0 - i64::from(s.1 <= 2), i64::from(s.1), i64::from(s.2));
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    usize::try_from((days + 3).rem_euclid(7)).unwrap_or(0)
}

/// Days between two dates (`b` less `a`), ignoring the time of day.
pub fn days_between(a: Stamp, b: Stamp) -> i64 {
    let day = |s: Stamp| {
        let (y, m, d) = (s.0 - i64::from(s.1 <= 2), i64::from(s.1), i64::from(s.2));
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
        era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy
    };
    day(b) - day(a)
}

/// A stem `unique_stem` made: its prefix, its time and any `(n)` after it
/// (`Sunday Service 2026-11-22 14-03-10 (2)` → `("Sunday Service", …, " (2)")`).
pub fn parse_stem(stem: &str) -> Option<(String, Stamp, String)> {
    let (head, tail) = match stem.rfind(" (") {
        Some(i) if stem.ends_with(')') && stem.get(i + 2..stem.len() - 1).is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) => {
            (stem.get(..i)?, stem.get(i..)?)
        }
        _ => (stem, ""),
    };
    // `<prefix> YYYY-MM-DD HH-MM-SS`: the last 19 characters.
    let cut = head.len().checked_sub(19)?;
    let (prefix, when) = (head.get(..cut)?, head.get(cut..)?);
    let b = when.as_bytes();
    let digits = |r: std::ops::Range<usize>| -> Option<i64> {
        let s = when.get(r)?;
        if s.bytes().all(|c| c.is_ascii_digit()) { s.parse().ok() } else { None }
    };
    if b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') || b.get(10) != Some(&b' ') || b.get(13) != Some(&b'-') || b.get(16) != Some(&b'-') {
        return None;
    }
    let small = |r| digits(r).and_then(|v| u32::try_from(v).ok());
    let s: Stamp = (digits(0..4)?, small(5..7)?, small(8..10)?, small(11..13)?, small(14..16)?, small(17..19)?);
    if !(1..=12).contains(&s.1) || !(1..=31).contains(&s.2) || s.3 > 23 || s.4 > 59 || s.5 > 60 {
        return None;
    }
    let prefix = prefix.strip_suffix(' ')?.to_string();
    (!prefix.trim().is_empty()).then_some((prefix, s, tail.to_string()))
}

/// Seconds since 1970 as a civil date and time (UTC).
pub fn stamp_of(secs: u64) -> Stamp {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, (rem / 3600) as u32, ((rem / 60) % 60) as u32, (rem % 60) as u32)
}

/// `2026-11-22 14-03-10` (no colons: Windows file names cannot hold them).
pub fn stamp_text(s: Stamp) -> String {
    format!("{:04}-{:02}-{:02} {:02}-{:02}-{:02}", s.0, s.1, s.2, s.3, s.4, s.5)
}

/// `name` made safe as a file name on every system (no `\ / : * ? " < > |`, no trailing dots or
/// spaces), at most 80 characters.
pub fn file_safe(name: &str) -> String {
    let s: String = name.chars().map(|c| if r#"\/:*?"<>|"#.contains(c) || c.is_control() { '-' } else { c }).take(80).collect();
    s.trim().trim_end_matches('.').trim().to_string()
}

/// A stem in `dir` that no `.wav` or `.mid` there has yet: `<prefix> <time>`, then `… (2)`,
/// `… (3)`… for takes in the same second. Never overwrites a file.
pub fn unique_stem(dir: &Path, prefix: &str, at: Stamp) -> String {
    let base = file_safe(&format!("{prefix} {}", stamp_text(at)));
    let taken = |stem: &str| ["wav", "mid"].iter().any(|ext| dir.join(format!("{stem}.{ext}")).exists());
    (1..10_000).map(|n| if n == 1 { base.clone() } else { format!("{base} ({n})") }).find(|s| !taken(s)).unwrap_or_else(|| format!("{base} (more)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_civil() {
        assert_eq!(stamp_of(0), (1970, 1, 1, 0, 0, 0));
        // 2026-11-22 14:03:10 UTC.
        assert_eq!(stamp_text(stamp_of(1_795_356_190)), "2026-11-22 14-03-10");
        assert_eq!(stamp_of(951_782_400), (2000, 2, 29, 0, 0, 0));
    }

    #[test]
    fn names_are_in_local_time() {
        let now = now_secs();
        let offset = soundcraft_sysmem::utc_offset_secs(now).unwrap_or(0);
        eprintln!("this computer is {offset} s ahead of UTC");
        assert_eq!(local_stamp(now), stamp_of((now + i64::from(offset)) as u64));
        // A take's name holds that local time, and reads back from it.
        let at = local_stamp(1_795_356_190);
        let stem = format!("Idea {}", stamp_text(at));
        assert_eq!(parse_stem(&stem), Some(("Idea".to_string(), at, String::new())));
        assert_eq!(parse_stem("Sunday Service 2026-11-22 14-03-10 (2)").map(|p| p.2), Some(" (2)".to_string()));
        assert_eq!(parse_stem("Chorus idea"), None);
        assert_eq!(parse_stem("Idea 2026-13-22 14-03-10"), None);
        assert_eq!(weekday((2026, 11, 23, 0, 0, 0)), 0, "a Monday");
        assert_eq!(weekday((2000, 2, 29, 0, 0, 0)), 1, "a Tuesday");
        assert_eq!(days_between((2026, 2, 28, 23, 0, 0), (2026, 3, 1, 1, 0, 0)), 1);
    }

    #[test]
    fn names_are_safe() {
        assert_eq!(file_safe("a/b:c?. "), "a-b-c-");
        assert_eq!(recorder_name("STUDIO-PC"), "STUDIO-PC Recorder");
        assert_eq!(recorder_name(" "), "Recorder");
    }

    #[test]
    fn prefs_round_trip_and_default() {
        let dir = std::env::temp_dir().join(format!("freedawm-recorder-prefs-{}", std::process::id()));
        let p = Paths::in_dir(&dir);
        assert_eq!(Prefs::load(&p.prefs), Prefs::default());
        let mut prefs = Prefs { channel: 2, stereo: true, ..Prefs::default() };
        prefs.midi.insert("Keys".into(), false);
        prefs.save(&p.prefs).unwrap();
        assert_eq!(Prefs::load(&p.prefs), prefs);
        let _ = std::fs::remove_dir_all(dir);
    }
}
