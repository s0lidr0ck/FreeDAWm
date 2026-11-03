//! The CLAP and VST3 plugin scan, in the background: a worker thread drives the hosts' scans
//! (each runs the `freedawm-scan` helper over a few bundles per child process, see
//! `soundcraft-plugin-scan`), while the browser shows "Scanning 312 / 767…" with Cancel and the
//! menus keep listing the plugins already known. A plugin looked up and missing from the scan
//! cache (a session opened with plugins never scanned) starts one too ("Resolving plugins…"); when
//! it ends, the plugins that could not be created are created again.

use crate::SoundApp;
use soundcraft_vst3_host::{ScanOptions, ScanProgress};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// A scan running on its worker thread.
pub struct ScanJob {
    progress: Arc<ScanProgress>,
    outcome: Arc<Mutex<Option<Result<(), String>>>>,
    #[cfg(not(target_arch = "wasm32"))]
    worker: Option<std::thread::JoinHandle<()>>,
    /// How many formats plan their work before the counts mean anything.
    formats: usize,
    /// Started because plugins were missing, not by the user.
    resolving: bool,
}

impl ScanJob {
    /// Scans the CLAP, then the VST3 plugins (new and changed bundles; with `retry_failed`, also
    /// the ones that failed before).
    pub fn start(retry_failed: bool) -> Self {
        let mut job = Self::start_with(move |p| scan_hosts(retry_failed, p));
        job.formats = 2;
        job
    }

    /// A scan started because plugins were looked up and missing.
    fn resolve() -> Self {
        let mut job = Self::start(false);
        job.resolving = true;
        job
    }

    /// Runs `work` as the scan (tests run a fake one).
    pub fn start_with(work: impl FnOnce(&ScanProgress) -> Result<(), String> + Send + 'static) -> Self {
        let progress = Arc::new(ScanProgress::default());
        let outcome = Arc::new(Mutex::new(None));
        let (p, o) = (Arc::clone(&progress), Arc::clone(&outcome));
        let run = move || {
            let r = work(&p);
            *o.lock().unwrap_or_else(PoisonError::into_inner) = Some(r);
        };
        #[cfg(not(target_arch = "wasm32"))]
        let worker = match std::thread::Builder::new().name("plugin-scan".into()).spawn(run) {
            Ok(h) => Some(h),
            Err(e) => {
                *outcome.lock().unwrap_or_else(PoisonError::into_inner) = Some(Err(format!("cannot start the plugin scan: {e}")));
                None
            }
        };
        #[cfg(target_arch = "wasm32")]
        run();
        *running_slot() = Some(Arc::clone(&progress));
        ScanJob {
            progress,
            outcome,
            #[cfg(not(target_arch = "wasm32"))]
            worker,
            formats: 0,
            resolving: false,
        }
    }

    /// Whether every format has planned its work (the total is final).
    pub fn counted(&self) -> bool {
        self.progress.planned() >= self.formats
    }

    /// Whether it runs because plugins were missing ("Resolving plugins…").
    pub fn resolving(&self) -> bool {
        self.resolving
    }

    /// Cancels the scan and waits (at most `wait`) for its worker, which stops the running
    /// helper process: for quitting the app.
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    pub fn stop(mut self, wait: std::time::Duration) {
        self.cancel();
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(h) = self.worker.take() {
            let until = std::time::Instant::now() + wait;
            while !h.is_finished() && std::time::Instant::now() < until {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if h.is_finished() {
                let _ = h.join();
            } else {
                log::warn!("plugin scan: the worker did not stop in {wait:?}");
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = wait;
        *running_slot() = None;
    }

    /// Bundles scanned and to scan so far.
    pub fn progress(&self) -> (usize, usize) {
        (self.progress.done(), self.progress.total())
    }

    /// Stops the scan (the helper running now is stopped; what was scanned is kept).
    pub fn cancel(&self) {
        self.progress.cancel();
    }

    pub fn cancelled(&self) -> bool {
        self.progress.cancelled()
    }

    /// The scan's result once it has ended.
    pub fn take_outcome(&self) -> Option<Result<(), String>> {
        self.outcome.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

fn scan_hosts(retry_failed: bool, progress: &ScanProgress) -> Result<(), String> {
    if retry_failed {
        soundcraft_clap_host::clear_failed();
        soundcraft_vst3_host::clear_failed();
    }
    let opts = ScanOptions { retry_failed, ..ScanOptions::default() };
    // Both formats at once: each plans its work (adding to the total) before it scans, so the
    // count shown covers both from the start.
    #[cfg(not(target_arch = "wasm32"))]
    let (clap, vst3) = std::thread::scope(|s| {
        let clap = s.spawn(|| soundcraft_clap_host::scan_with(&opts, progress));
        let vst3 = soundcraft_vst3_host::scan_with(&opts, progress);
        (clap.join().unwrap_or_else(|_| Err("the CLAP scan failed".into())), vst3)
    });
    #[cfg(target_arch = "wasm32")]
    let (clap, vst3) = (soundcraft_clap_host::scan_with(&opts, progress), soundcraft_vst3_host::scan_with(&opts, progress));
    if progress.cancelled() {
        return Err("Plugin scan cancelled".into());
    }
    clap.and(vst3).map(|_| ())
}

/// The running scan's progress, for the menus (which have no app at hand).
fn running_slot() -> std::sync::MutexGuard<'static, Option<Arc<ScanProgress>>> {
    static RUNNING: Mutex<Option<Arc<ScanProgress>>> = Mutex::new(None);
    RUNNING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A scan asked for by a menu: 0 none, 1 scan, 2 scan retrying failed plugins.
static REQUESTED: AtomicU8 = AtomicU8::new(0);

/// Asks for a scan (started on the next frame unless one is running).
pub fn request(retry_failed: bool) {
    REQUESTED.store(if retry_failed { 2 } else { 1 }, Ordering::Relaxed);
}

/// The running scan's (done, total), if one runs.
pub fn running() -> Option<(usize, usize)> {
    running_slot().as_ref().map(|p| (p.done(), p.total()))
}

/// Starts a scan unless one runs.
pub(crate) fn start(app: &mut SoundApp, retry_failed: bool) {
    if app.plugin_scan.is_none() {
        app.plugin_scan = Some(ScanJob::start(retry_failed));
        app.ui.status = "Scanning plugins…".into();
    }
}

/// Each frame: starts a requested scan (or one resolving plugins that were looked up and missing),
/// and when the running one ends, creates the missing plugins again, rebuilds the browser's
/// catalog and reports in the status line.
pub(crate) fn step(app: &mut SoundApp, ctx: &egui::Context) {
    match REQUESTED.swap(0, Ordering::Relaxed) {
        0 => {}
        r => start(app, r == 2),
    }
    if app.plugin_scan.is_none() {
        // Both asked (no short circuit): each clears its own request.
        let clap = soundcraft_clap_host::take_lookup_request();
        let vst3 = soundcraft_vst3_host::take_lookup_request();
        if clap || vst3 {
            app.plugin_scan = Some(ScanJob::resolve());
            app.ui.status = "Resolving plugins…".into();
        }
    }
    let Some(job) = &app.plugin_scan else { return };
    let Some(outcome) = job.take_outcome() else {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        return;
    };
    let resolving = job.resolving();
    app.plugin_scan = None;
    *running_slot() = None;
    // Plugins the scan may have found: the ones that could not be created are tried again.
    if let Some(p) = &app.player {
        p.retry_missing_plugins(app.engine.session_arc());
    }
    super::panel::rebuild_catalog(app);
    let n = app.browser_catalog.as_ref().map_or(0, |c| c.items.len());
    app.ui.status = match outcome {
        Ok(()) if resolving => "Plugins resolved".to_string(),
        Ok(()) => format!("Plugin browser: {n} plugins"),
        Err(e) => {
            log::warn!("plugin scan: {e}");
            e
        }
    };
}

impl SoundApp {
    /// Stops a running plugin scan and its helper process (on quit).
    pub fn stop_plugin_scan(&mut self) {
        if let Some(job) = self.plugin_scan.take() {
            job.stop(std::time::Duration::from_secs(3));
        }
    }
}
