//! The ARA log: one line per notable ARA event (a document made or refused, an edit cycle, a
//! plug-in programming error it reported through the ARA assert function, a read that failed),
//! written to the file [`set_log_file`] names (the app uses `<config>/Logs/ara.log`) and kept in
//! memory (the last [`KEPT`] lines, [`recent`]) for tests and the UI. Never written from the
//! audio thread (nothing in ARA runs there).

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

/// Lines kept in memory.
pub const KEPT: usize = 512;
/// The log file is started afresh when it grows past this.
const MAX_FILE_BYTES: u64 = 4 << 20;

struct Log {
    file: Option<PathBuf>,
    lines: VecDeque<String>,
}

fn log() -> &'static Mutex<Log> {
    static L: Mutex<Log> = Mutex::new(Log { file: None, lines: VecDeque::new() });
    &L
}

/// Where ARA log lines are appended (`None`: memory only, the default). The folder is made.
pub fn set_log_file(path: Option<PathBuf>) {
    if let Some(dir) = path.as_ref().and_then(|p| p.parent()) {
        let _ = std::fs::create_dir_all(dir);
    }
    log().lock().unwrap_or_else(PoisonError::into_inner).file = path;
}

/// The log file set with [`set_log_file`].
pub fn log_file() -> Option<PathBuf> {
    log().lock().unwrap_or_else(PoisonError::into_inner).file.clone()
}

/// The last lines logged (oldest first).
pub fn recent() -> Vec<String> {
    log().lock().unwrap_or_else(PoisonError::into_inner).lines.iter().cloned().collect()
}

/// Logs one line (also to standard error in debug builds).
pub fn line(text: &str) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    let text: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).take(2000).collect();
    let entry = format!("{secs:.3} {text}");
    if cfg!(debug_assertions) {
        eprintln!("ara: {text}");
    }
    let mut l = log().lock().unwrap_or_else(PoisonError::into_inner);
    if l.lines.len() >= KEPT {
        l.lines.pop_front();
    }
    l.lines.push_back(entry.clone());
    if let Some(path) = l.file.clone() {
        let fresh = std::fs::metadata(&path).map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(false);
        let file = if fresh { std::fs::File::create(&path) } else { std::fs::OpenOptions::new().create(true).append(true).open(&path) };
        if let Ok(mut f) = file {
            let _ = writeln!(f, "{entry}");
        }
    }
}

/// `format!`-style logging to the ARA log.
#[macro_export]
#[doc(hidden)]
macro_rules! ara_log {
    ($($t:tt)*) => {
        $crate::log::line(&format!($($t)*))
    };
}
