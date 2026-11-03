//! The scanner helper (`freedawm-scan --format vst3|clap --out <file> <bundle>...`): its
//! arguments, the results file it writes (one JSON line per bundle, flushed as it goes, then a
//! `done` line) and the runner that starts it, watches the file and stops it when it hangs.

use crate::{BatchEnd, BatchResult, BatchRunner, HELPER_NAME};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// How often the runner looks at the helper.
const POLL: Duration = Duration::from_millis(20);
/// How long a helper that reported everything may take to exit (plugin module exits) before it
/// is stopped.
const EXIT_GRACE: Duration = Duration::from_secs(2);

/// One line of the results file.
#[derive(Serialize, Deserialize)]
#[serde(bound(serialize = "D: Serialize", deserialize = "D: DeserializeOwned"))]
struct Line<D> {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plugins: Option<Vec<D>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    done: bool,
}

/// The helper's command line.
#[derive(Debug, Clone, PartialEq)]
pub struct HelperArgs {
    /// `vst3` or `clap`.
    pub format: String,
    /// The results file.
    pub out: PathBuf,
    pub bundles: Vec<PathBuf>,
}

/// Parses `--format <f> --out <file> <bundle>...` (arguments after the program name).
pub fn parse_helper_args(args: impl IntoIterator<Item = OsString>) -> Result<HelperArgs, String> {
    let mut it = args.into_iter();
    let (mut format, mut out, mut bundles) = (None, None, Vec::new());
    while let Some(a) = it.next() {
        match a.to_str() {
            Some("--format") => format = it.next().and_then(|f| f.into_string().ok()),
            Some("--out") => out = it.next().map(PathBuf::from),
            _ => bundles.push(PathBuf::from(a)),
        }
    }
    let usage = "usage: freedawm-scan --format vst3|clap --out <results file> <bundle>...";
    match (format, out) {
        (Some(format), Some(out)) if format == "vst3" || format == "clap" => Ok(HelperArgs { format, out, bundles }),
        _ => Err(usage.to_string()),
    }
}

/// The helper's side of the results file.
pub struct ResultWriter {
    file: std::fs::File,
}

impl ResultWriter {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        Ok(ResultWriter { file: std::fs::File::create(path)? })
    }

    /// Reports one bundle (written and flushed at once: the process may die on the next one).
    pub fn bundle<D: Serialize>(&mut self, path: &Path, result: Result<Vec<D>, String>) -> std::io::Result<()> {
        let (plugins, error) = match result {
            Ok(p) => (Some(p), None),
            Err(e) => (None, Some(e)),
        };
        self.line(&Line { path: Some(path.to_path_buf()), plugins, error, done: false })
    }

    /// Reports that every bundle was reported.
    pub fn done(&mut self) -> std::io::Result<()> {
        self.line(&Line::<()> { path: None, plugins: None, error: None, done: true })
    }

    fn line<D: Serialize>(&mut self, line: &Line<D>) -> std::io::Result<()> {
        let mut text = serde_json::to_string(line).map_err(std::io::Error::other)?;
        text.push('\n');
        self.file.write_all(text.as_bytes())?;
        self.file.flush()?;
        self.file.sync_data()
    }
}

/// The helper next to the running program (`FREEDAWM_SCAN_HELPER`, or the older
/// `SOUNDCRAFT_SCAN_HELPER`, names another), or why there
/// is none.
pub fn helper_exe() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("FREEDAWM_SCAN_HELPER").or_else(|| std::env::var_os("SOUNDCRAFT_SCAN_HELPER")).filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        return if p.is_file() { Ok(p) } else { Err(format!("the plugin scanner {} (FREEDAWM_SCAN_HELPER) does not exist", p.display())) };
    }
    let exe = std::env::current_exe().map_err(|e| format!("cannot find the plugin scanner: {e}"))?;
    let name = format!("{HELPER_NAME}{}", std::env::consts::EXE_SUFFIX);
    let helper = exe.parent().map(|d| d.join(&name)).unwrap_or_else(|| PathBuf::from(&name));
    if helper.is_file() {
        Ok(helper)
    } else {
        Err(format!(
            "The plugin scanner ({name}) is missing: it must sit next to {} (build it with `cargo build -p soundcraft-scan`, or `cargo xtask build-asio`). Plugins are not scanned inside the app, because some crash it.",
            exe.file_name().map_or_else(|| "the app".into(), |n| n.to_string_lossy().into_owned())
        ))
    }
}

/// Results files older than this are left over from a process that died: removed.
const STALE_RESULTS: Duration = Duration::from_secs(3600);

/// Runs batches through the helper executable, one child process each.
pub struct HelperRunner {
    exe: PathBuf,
    format: String,
    on_spawn: Option<fn(&Child)>,
}

impl HelperRunner {
    pub fn new(exe: PathBuf, format: &str) -> Self {
        remove_stale_results();
        HelperRunner { exe, format: format.to_string(), on_spawn: None }
    }

    /// Calls `hook` with every helper started (the hosts tie it to this process, so that it dies
    /// with it).
    pub fn on_spawn(mut self, hook: fn(&Child)) -> Self {
        self.on_spawn = Some(hook);
        self
    }

    fn results_file(&self) -> PathBuf {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("{HELPER_NAME}-{}-{}-{n}.jsonl", self.format, std::process::id()))
    }

    fn spawn(&self, out: &Path, paths: &[PathBuf]) -> std::io::Result<Child> {
        let mut cmd = Command::new(&self.exe);
        cmd.arg("--format").arg(&self.format).arg("--out").arg(out).args(paths);
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console window flashes up from the GUI app; and
            // BELOW_NORMAL_PRIORITY_CLASS: a scan while playing does not compete with the audio.
            cmd.creation_flags(0x0800_0000 | 0x0000_4000);
        }
        let child = cmd.spawn()?;
        if let Some(hook) = self.on_spawn {
            hook(&child);
        }
        Ok(child)
    }
}

/// Removes results files that processes which died (a crash, a kill) left in the temp folder.
fn remove_stale_results() {
    let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for e in rd.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with(&format!("{HELPER_NAME}-")) && name.ends_with(".jsonl")) {
            continue;
        }
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age > STALE_RESULTS);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Reads the complete lines added to the results file since the last call.
struct Tail {
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
}

impl Tail {
    fn lines<D: DeserializeOwned>(&mut self) -> Vec<Line<D>> {
        let Ok(mut f) = std::fs::File::open(&self.path) else { return Vec::new() };
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut more = Vec::new();
        let Ok(n) = f.read_to_end(&mut more) else { return Vec::new() };
        self.offset += n as u64;
        self.partial.extend_from_slice(&more);
        let Some(end) = self.partial.iter().rposition(|b| *b == b'\n') else { return Vec::new() };
        let complete: Vec<u8> = self.partial.drain(..=end).collect();
        String::from_utf8_lossy(&complete).lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }
}

fn describe(status: std::process::ExitStatus) -> String {
    match status.code() {
        // Windows exceptions read best in hex (0xC0000005 is an access violation).
        Some(c) if cfg!(windows) => format!("exit code {:#010X}", c as u32),
        Some(c) => format!("exit code {c}"),
        None => format!("{status}"),
    }
}

fn stop(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

impl<D: DeserializeOwned> BatchRunner<D> for HelperRunner {
    fn check(&mut self, cancel: &AtomicBool) -> Result<(), String> {
        let r: BatchResult<D> = self.run(&[], Duration::from_secs(20), cancel);
        match r.end {
            BatchEnd::Finished => Ok(()),
            BatchEnd::Crashed(why) => Err(format!("the plugin scanner {} does not work ({why})", self.exe.display())),
            BatchEnd::TimedOut => Err(format!("the plugin scanner {} does not answer", self.exe.display())),
            BatchEnd::Cancelled => Ok(()),
        }
    }

    fn run(&mut self, paths: &[PathBuf], per_bundle: Duration, cancel: &AtomicBool) -> BatchResult<D> {
        let out = self.results_file();
        let _ = std::fs::remove_file(&out);
        let mut results = Vec::new();
        let mut child = match self.spawn(&out, paths) {
            Ok(c) => c,
            Err(e) => return BatchResult { results, end: BatchEnd::Crashed(format!("cannot start {}: {e}", self.exe.display())) },
        };
        let mut tail = Tail { path: out.clone(), offset: 0, partial: Vec::new() };
        let mut last = Instant::now();
        let mut done_at: Option<Instant> = None;
        let end = loop {
            let exited = child.try_wait().ok().flatten();
            // Read after checking for exit, so that nothing written before it is missed.
            for line in tail.lines::<D>() {
                if line.done {
                    done_at.get_or_insert_with(Instant::now);
                } else if let Some(path) = line.path {
                    last = Instant::now();
                    results.push((path, line.plugins.ok_or_else(|| line.error.unwrap_or_else(|| "no result".into()))));
                }
            }
            match (exited, done_at) {
                (Some(_), Some(_)) => break BatchEnd::Finished,
                (Some(status), None) => break BatchEnd::Crashed(describe(status)),
                (None, Some(at)) if at.elapsed() > EXIT_GRACE => {
                    // Everything is reported; a plugin's module exit is holding the process up.
                    stop(&mut child);
                    break BatchEnd::Finished;
                }
                _ => {}
            }
            if cancel.load(Ordering::Relaxed) {
                stop(&mut child);
                break BatchEnd::Cancelled;
            }
            if done_at.is_none() && last.elapsed() > per_bundle {
                stop(&mut child);
                break BatchEnd::TimedOut;
            }
            std::thread::sleep(POLL);
        };
        let _ = std::fs::remove_file(&out);
        BatchResult { results, end }
    }
}
