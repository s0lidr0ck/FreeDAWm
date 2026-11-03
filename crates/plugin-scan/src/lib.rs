//! Plugin scanning out of process, shared by SoundCraft's CLAP and VST3 hosts.
//!
//! Listing installed plugins means loading every plugin binary, and some binaries crash or hang
//! the process that loads them — sometimes only after many others were loaded into it (one
//! machine with 767 VST3 bundles died around the 676th, whichever it was). So the app never loads
//! them to list them: the `freedawm-scan` helper does, a few bundles per process, and this crate
//! drives it:
//!
//! - [`run_scan`] plans the scan from the [`ScanCache`] (only new or changed bundles, by
//!   modification time and size; failed ones only when asked to retry them), runs the helper over
//!   batches ([`ScanOptions::batch`], 16 by default) through a [`BatchRunner`] and records each
//!   bundle's plugins or failure.
//! - A batch whose process crashes or stops answering ([`ScanOptions::per_bundle`], 20 s without a
//!   result) keeps the results it reported; the bundle it was loading is tried again alone in a
//!   fresh process (allowed [`ScanOptions::solo_timeout`], 60 s), and the rest of the batch in
//!   another. A bundle is only marked failed ("crashed during scan", "timed out during scan") when
//!   it fails alone, so a process poisoned by earlier bundles never blames an innocent one. Failed
//!   bundles are skipped by later scans until they change or the user rescans retrying failed
//!   plugins.
//! - [`HelperRunner`] is the real runner (a child process per batch, results read from its
//!   output file as they arrive); the helper side writes with [`ResultWriter`].
//!
//! No `unsafe`, no plugin code: this crate only spawns processes and reads and writes JSON.

mod cache;
mod helper;

pub use cache::{CacheLock, Entry, FailKind, Outcome, ScanCache, Stamp, file_stamp};
pub use helper::{HelperArgs, HelperRunner, ResultWriter, helper_exe, parse_helper_args};

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

/// The helper's executable name (without the platform's suffix).
pub const HELPER_NAME: &str = "freedawm-scan";

/// A plugin a scan lists: what the cache needs to know of a host's descriptor.
pub trait Descriptor: Clone + Serialize + DeserializeOwned + Send + 'static {
    /// The SoundCraft plugin id (`vst3:…`, `clap:…`): the first bundle listing an id wins.
    fn id(&self) -> &str;
    /// The display name (lists are sorted by it).
    fn name(&self) -> &str;
}

/// How a scan runs.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanOptions {
    /// Scan the bundles that failed before (crashed, timed out or would not load) again.
    pub retry_failed: bool,
    /// Bundles per helper process.
    pub batch: usize,
    /// How long a helper may go without reporting a bundle before it is stopped.
    pub per_bundle: Duration,
    /// The same, for a bundle retried alone after its batch's process crashed or hung (a slow
    /// bundle gets more time before it is blamed).
    pub solo_timeout: Duration,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions { retry_failed: false, batch: 16, per_bundle: Duration::from_secs(20), solo_timeout: Duration::from_secs(60) }
    }
}

/// A running scan's progress, shared with the thread that shows it, and its Cancel. Several
/// scans (CLAP and VST3) may add to the same progress: each adds its planned bundles to the total
/// and counts itself planned, so a display can wait until every format has planned.
#[derive(Debug, Default)]
pub struct ScanProgress {
    done: AtomicUsize,
    total: AtomicUsize,
    planned: AtomicUsize,
    cancel: AtomicBool,
    crashes: AtomicUsize,
    timeouts: AtomicUsize,
}

impl ScanProgress {
    /// Bundles scanned so far.
    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }
    /// Bundles to scan (grows as each format's scan plans its work).
    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }
    /// Asks the scan to stop: the running helper is stopped and what was scanned is kept.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    /// The cancel flag, for a [`BatchRunner`].
    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.cancel
    }
    pub fn add_total(&self, n: usize) {
        self.total.fetch_add(n, Ordering::Relaxed);
    }
    /// How many scans have planned their work (and added it to the total).
    pub fn planned(&self) -> usize {
        self.planned.load(Ordering::Relaxed)
    }
    pub fn mark_planned(&self) {
        self.planned.fetch_add(1, Ordering::Relaxed);
    }
    pub fn step(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }
    /// Helper processes that died so far (each was followed by a retry in fresh processes).
    pub fn crashes(&self) -> usize {
        self.crashes.load(Ordering::Relaxed)
    }
    /// Helper processes stopped for hanging so far.
    pub fn timeouts(&self) -> usize {
        self.timeouts.load(Ordering::Relaxed)
    }
}

/// Why a scan stopped early.
#[derive(Debug, Clone, PartialEq)]
pub enum ScanError {
    /// The user cancelled it (what was scanned is kept).
    Cancelled,
    /// The helper is missing or does not work; nothing was scanned.
    Helper(String),
}

impl std::fmt::Display for ScanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScanError::Cancelled => write!(f, "plugin scan cancelled"),
            ScanError::Helper(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for ScanError {}

/// How a helper process ended.
#[derive(Debug, Clone, PartialEq)]
pub enum BatchEnd {
    /// It reported every bundle.
    Finished,
    /// It died (the reason: exit code or start failure).
    Crashed(String),
    /// It went [`ScanOptions::per_bundle`] without reporting a bundle and was stopped.
    TimedOut,
    /// The scan was cancelled and the process stopped.
    Cancelled,
}

/// What one helper process reported, in the order it loaded the bundles, and how it ended.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchResult<D> {
    /// Each bundle reported: its plugins, or why it would not load.
    pub results: Vec<(PathBuf, Result<Vec<D>, String>)>,
    pub end: BatchEnd,
}

/// Runs one batch of bundles through a scanner process (the real one is [`HelperRunner`]).
pub trait BatchRunner<D> {
    /// Checks that the scanner works at all (a process with no bundles), so that a broken helper
    /// is reported once rather than as every bundle crashing.
    fn check(&mut self, _cancel: &AtomicBool) -> Result<(), String> {
        Ok(())
    }
    /// Loads `paths`, in order, in one fresh process.
    fn run(&mut self, paths: &[PathBuf], per_bundle: Duration, cancel: &AtomicBool) -> BatchResult<D>;
}

/// Scans `found` (every bundle in the search folders now) into `cache`: drops bundles that are
/// gone, plans the new and changed ones (and failed ones on [`ScanOptions::retry_failed`]), and
/// runs them in batches with crash and hang isolation (see the crate docs). `after_batch` sees the
/// cache after each batch (to save it and show the plugins found so far). On cancel, what was
/// scanned is kept.
pub fn run_scan<D: Descriptor>(
    cache: &mut ScanCache<D>,
    found: &[PathBuf],
    stamp: &dyn Fn(&Path) -> Option<Stamp>,
    runner: &mut dyn BatchRunner<D>,
    opts: &ScanOptions,
    progress: &ScanProgress,
    after_batch: &mut dyn FnMut(&ScanCache<D>),
) -> Result<(), ScanError> {
    cache.retain_found(found);
    let stamps: Vec<(PathBuf, Option<Stamp>)> = found.iter().map(|p| (p.clone(), stamp(p))).collect();
    let todo = cache.plan(&stamps, opts.retry_failed);
    progress.add_total(todo.len());
    progress.mark_planned();
    if todo.is_empty() {
        after_batch(cache);
        return Ok(());
    }
    runner.check(progress.cancel_flag()).map_err(ScanError::Helper)?;
    if progress.cancelled() {
        return Err(ScanError::Cancelled);
    }
    let stamp_of = |p: &Path| stamps.iter().find(|(q, _)| q == p).and_then(|(_, s)| *s);
    for chunk in todo.chunks(opts.batch.max(1)) {
        if progress.cancelled() {
            after_batch(cache);
            return Err(ScanError::Cancelled);
        }
        let complete = isolate(chunk, runner, opts, progress, &mut |path, outcome| {
            let s = stamp_of(&path);
            cache.record(path, s, outcome);
            progress.step();
        });
        after_batch(cache);
        if !complete {
            return Err(ScanError::Cancelled);
        }
    }
    Ok(())
}

/// Runs `paths` through fresh processes until every bundle has an outcome; `false` if cancelled.
fn isolate<D>(
    paths: &[PathBuf],
    runner: &mut dyn BatchRunner<D>,
    opts: &ScanOptions,
    progress: &ScanProgress,
    record: &mut dyn FnMut(PathBuf, Outcome<D>),
) -> bool {
    // Each set to run, and whether it is a suspect retried alone (it gets the longer timeout).
    let mut queue: VecDeque<(Vec<PathBuf>, bool)> = VecDeque::from([(paths.to_vec(), false)]);
    while let Some((set, solo)) = queue.pop_front() {
        if set.is_empty() {
            continue;
        }
        if progress.cancelled() {
            return false;
        }
        let timeout = if solo { opts.solo_timeout.max(opts.per_bundle) } else { opts.per_bundle };
        let r = runner.run(&set, timeout, progress.cancel_flag());
        let mut rest = set.clone();
        let mut reported = 0;
        for (path, result) in r.results {
            let Some(at) = rest.iter().position(|p| *p == path) else { continue };
            rest.remove(at);
            reported += 1;
            record(
                path,
                match result {
                    Ok(plugins) => Outcome::Plugins(plugins),
                    Err(reason) => Outcome::Failed { reason, kind: FailKind::Load },
                },
            );
        }
        let failed = |end: &BatchEnd| match end {
            BatchEnd::TimedOut => Some((FailKind::TimedOut, "timed out during scan".to_string())),
            BatchEnd::Crashed(why) => Some((FailKind::Crashed, format!("crashed during scan ({why})"))),
            _ => None,
        };
        match (&r.end, rest.split_first()) {
            (_, None) => {}
            (BatchEnd::Cancelled, _) => return false,
            (BatchEnd::Finished, Some(_)) => {
                for p in rest {
                    record(p, Outcome::Failed { reason: "the scanner reported nothing for it".into(), kind: FailKind::Load });
                }
            }
            (end, Some((suspect, others))) => {
                let counter = if *end == BatchEnd::TimedOut { &progress.timeouts } else { &progress.crashes };
                counter.fetch_add(1, Ordering::Relaxed);
                log::info!("plugin scan: the scanner {end:?} while loading {} ({} of {} in its process)", suspect.display(), reported + 1, set.len());
                if set.len() == 1 && reported == 0 {
                    // It failed alone, in a fresh process: it is the culprit.
                    if let Some((kind, reason)) = failed(end) {
                        log::warn!("plugin scan: {} {reason}", suspect.display());
                        record(suspect.clone(), Outcome::Failed { reason, kind });
                    }
                } else {
                    // The process died loading `suspect`, maybe poisoned by what it loaded
                    // before: try it alone, then the rest in another fresh process.
                    queue.push_front((others.to_vec(), false));
                    queue.push_front((vec![suspect.clone()], true));
                }
            }
        }
    }
    true
}
