//! The scan cache: each bundle's plugins or failure, with the modification time and size it had
//! when scanned, saved as JSON (`<config>/PluginScan/vst3.json`, `clap.json`).

use crate::Descriptor;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// The cache file's format version: a file of another version is ignored (scanned again).
const VERSION: u32 = 1;

/// A bundle's modification time and size when it was scanned: a bundle whose stamp changed (an
/// update was installed) is scanned again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    /// Milliseconds since the Unix epoch.
    pub modified_ms: u64,
    pub size: u64,
}

/// The stamp of a file (or folder) as it is now; `None` if it cannot be read.
pub fn file_stamp(path: &Path) -> Option<Stamp> {
    let md = std::fs::metadata(path).ok()?;
    let modified_ms = md.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_millis();
    Some(Stamp { modified_ms: u64::try_from(modified_ms).unwrap_or(u64::MAX), size: md.len() })
}

/// Why a bundle is not listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailKind {
    /// It loaded and said it is not a plugin, or reported an error.
    Load,
    /// Its scanner process died while loading it alone.
    Crashed,
    /// Its scanner process stopped answering while loading it alone.
    TimedOut,
}

/// A bundle's scan result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", bound(serialize = "D: Serialize", deserialize = "D: DeserializeOwned"))]
pub enum Outcome<D> {
    Plugins(Vec<D>),
    Failed { reason: String, kind: FailKind },
}

/// One bundle in the cache.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound(serialize = "D: Serialize", deserialize = "D: DeserializeOwned"))]
pub struct Entry<D> {
    pub path: PathBuf,
    pub stamp: Option<Stamp>,
    pub outcome: Outcome<D>,
}

/// Every scanned bundle, sorted by path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(bound(serialize = "D: Serialize", deserialize = "D: DeserializeOwned"))]
pub struct ScanCache<D> {
    pub version: u32,
    pub entries: Vec<Entry<D>>,
}

impl<D> Default for ScanCache<D> {
    fn default() -> Self {
        ScanCache { version: VERSION, entries: Vec::new() }
    }
}

impl<D: Descriptor> ScanCache<D> {
    /// Reads a cache file. A missing, unreadable or outdated file is an empty cache (logged).
    pub fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("plugin scan cache {}: {e}", path.display());
                }
                return Self::default();
            }
        };
        match serde_json::from_str::<Self>(&text) {
            Ok(c) if c.version == VERSION => c,
            Ok(c) => {
                log::info!("plugin scan cache {}: version {} ignored", path.display(), c.version);
                Self::default()
            }
            Err(e) => {
                log::warn!("plugin scan cache {}: {e}", path.display());
                Self::default()
            }
        }
    }

    /// Writes the cache file (creating its folder) through a temporary file, so a crash never
    /// leaves half a file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        // Per process: the app and the command line may save the same cache.
        let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    /// Records a bundle's result (replacing an earlier one).
    pub fn record(&mut self, path: PathBuf, stamp: Option<Stamp>, outcome: Outcome<D>) {
        let entry = Entry { path, stamp, outcome };
        match self.entries.binary_search_by(|e| e.path.cmp(&entry.path)) {
            Ok(at) => self.entries[at] = entry,
            Err(at) => self.entries.insert(at, entry),
        }
    }

    /// Forgets the bundles that are no longer installed.
    pub fn retain_found(&mut self, found: &[PathBuf]) {
        self.entries.retain(|e| found.contains(&e.path));
    }

    /// The bundles to scan: new ones, changed ones (stamp differs, or cannot be read) and, with
    /// `retry_failed`, failed ones. In the given order.
    pub fn plan(&self, found: &[(PathBuf, Option<Stamp>)], retry_failed: bool) -> Vec<PathBuf> {
        found
            .iter()
            .filter(|(p, stamp)| match self.entries.binary_search_by(|e| e.path.cmp(p)) {
                Err(_) => true,
                Ok(at) => {
                    let e = &self.entries[at];
                    stamp.is_none() || e.stamp != *stamp || (retry_failed && matches!(e.outcome, Outcome::Failed { .. }))
                }
            })
            .map(|(p, _)| p.clone())
            .collect()
    }

    /// Every plugin listed (the first bundle listing an id wins), sorted by name.
    pub fn plugins(&self) -> Vec<D> {
        let mut out: Vec<D> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for e in &self.entries {
            if let Outcome::Plugins(ps) = &e.outcome {
                for p in ps {
                    if seen.insert(p.id().to_string()) {
                        out.push(p.clone());
                    }
                }
            }
        }
        out.sort_by_cached_key(|d| d.name().to_lowercase());
        out
    }

    /// The bundles not listed, each with the reason.
    pub fn failures(&self) -> Vec<(PathBuf, String)> {
        self.entries
            .iter()
            .filter_map(|e| match &e.outcome {
                Outcome::Failed { reason, .. } => Some((e.path.clone(), reason.clone())),
                Outcome::Plugins(_) => None,
            })
            .collect()
    }

    /// The plugin with this id.
    pub fn find(&self, id: &str) -> Option<D> {
        self.entries.iter().find_map(|e| match &e.outcome {
            Outcome::Plugins(ps) => ps.iter().find(|p| p.id() == id).cloned(),
            Outcome::Failed { .. } => None,
        })
    }
}

/// A lock file next to a scan cache, held while a scan runs, so that the app and the command line
/// never scan into the same cache at once. It is an operating-system lock on the open file: the
/// system releases it when the process ends, however it ends (a crash leaves no stale lock).
#[derive(Debug)]
pub struct CacheLock {
    _file: std::fs::File,
}

impl CacheLock {
    /// Takes the lock at `path`, or says that another process holds it.
    pub fn acquire(path: &Path) -> Result<CacheLock, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(CacheLock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => Err("another FreeDAWm is scanning plugins right now; try again when it is done".to_string()),
            Err(std::fs::TryLockError::Error(e)) => Err(format!("{}: {e}", path.display())),
        }
    }
}
