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

/// Now, in UTC. (Local time needs the system's time zone, which the standard library does not
/// read; see `docs/recorder.md`.)
pub fn now_stamp() -> Stamp {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    stamp_of(secs)
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
