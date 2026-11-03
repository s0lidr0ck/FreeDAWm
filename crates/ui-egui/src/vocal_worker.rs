//! Vocal tuning's background host: runs the engine's vocal jobs (analyses, then tuned renders)
//! one at a time, off the UI thread on desktop and inline between frames on wasm32 (no threads).
//!
//! - `poll` once per frame: picks up a finished job and installs it, cancels a running render
//!   whose clip's tuning changed since it started (the next poll renders the newer edit), and
//!   only then asks for the next job. It never asks while a job is in flight.
//! - A job that fails (anything but a cancel) is marked failed in the engine so it isn't offered
//!   again, and leaves a message in the status bar. A result that installs as unused (its audio
//!   changed meanwhile) is only dropped: an Undo may want it again.

use soundcraft_engine::Engine;
use soundcraft_engine::vocal::{self, JobError, VocalDone, VocalJob};
use soundcraft_model::{AnalysisKey, ClipId, Session};
use soundcraft_time::Samples;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::Ordering;

/// What a job is, for the status bar and for cancelling a superseded render.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
struct Info {
    /// "Analysing Lead Vox…" / "Tuning Lead Vox…".
    label: String,
    /// The job's track name.
    track: String,
    /// A render's clip and render inputs (cancelled once the clip's inputs change).
    render: Option<(ClipId, u64)>,
    /// [`VocalJob::failure_key`].
    key: u64,
    /// The session the job was made in (`Engine::loads`).
    loads: u64,
}

#[cfg(not(target_arch = "wasm32"))]
struct Running {
    info: Info,
    cancel: Arc<AtomicBool>,
    rx: std::sync::mpsc::Receiver<std::result::Result<VocalDone, JobError>>,
}

/// See the module docs.
#[derive(Default)]
pub struct VocalWorker {
    #[cfg(not(target_arch = "wasm32"))]
    running: Option<Running>,
    /// The last failure worth telling (a render refused), until something installs.
    message: Option<String>,
    /// What the session was (`Engine::loads`, revision, the analyses' address, the focus) when
    /// `next_job` last found nothing to do: an idle worker doesn't re-scan an unchanged session
    /// every frame.
    idle_at: Option<(u64, u64, usize, Option<ClipId>)>,
    /// How many times `next_job` was asked (for the tests).
    scans: u64,
}

/// The track name of the first clip that hears `key` (an analysis job), or of `clip`.
fn track_of(s: &Session, clip: Option<ClipId>, key: Option<u64>) -> String {
    s.tracks
        .iter()
        .find(|t| {
            t.clips().iter().any(|c| match (clip, key) {
                (Some(id), _) => c.id == id,
                (None, Some(k)) => AnalysisKey::of(s, c).is_some_and(|a| a.hash() == k),
                (None, None) => false,
            })
        })
        .map_or_else(|| "vocal".to_string(), |t| t.name.clone())
}

fn info_of(e: &Engine, job: &VocalJob) -> Info {
    let s = e.session();
    let (track, label, render) = match job {
        VocalJob::Render(r) => {
            let t = track_of(s, Some(r.clip), None);
            (t.clone(), format!("Tuning {t}…"), Some((r.clip, r.inputs)))
        }
        VocalJob::Analyse { key, .. } => {
            let t = track_of(s, None, Some(key.hash()));
            (t.clone(), format!("Analysing {t}…"), None)
        }
    };
    Info { label, track, render, key: job.failure_key(), loads: e.loads() }
}

impl VocalWorker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Status-bar text: the job running ("Analysing Lead Vox…"), else the last failure worth
    /// telling, else `None`.
    pub fn busy(&self) -> Option<String> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(r) = &self.running {
            return Some(r.info.label.clone());
        }
        self.message.clone()
    }

    /// How many times the worker asked the engine for a job (for the tests).
    pub fn scans(&self) -> u64 {
        self.scans
    }

    /// The engine's next job, unless nothing changed since it last had none.
    fn next(&mut self, engine: &Engine, focus: Option<ClipId>, playhead: Samples) -> Option<VocalJob> {
        let state = (engine.loads(), engine.revision, Arc::as_ptr(&engine.session().vocal.analyses) as usize, focus);
        if self.idle_at == Some(state) {
            return None;
        }
        self.scans = self.scans.saturating_add(1);
        let job = vocal::next_job(engine, focus, playhead);
        self.idle_at = job.is_none().then_some(state);
        job
    }

    /// Whether a job is in flight (the host keeps polling while it is).
    pub fn running(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.running.is_some()
        }
        #[cfg(target_arch = "wasm32")]
        {
            false
        }
    }

    /// One frame's worth of work (see the module docs). `focus`: the clip open in the Vocal
    /// Editor, done first. Returns true when it installed something (repaint).
    pub fn poll(&mut self, engine: &mut Engine, focus: Option<ClipId>, playhead: Samples) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let installed = self.collect(engine);
            if self.running.is_none()
                && let Some(job) = self.next(engine, focus, playhead)
            {
                self.start(engine, job);
            }
            installed
        }
        #[cfg(target_arch = "wasm32")]
        {
            let Some(job) = self.next(engine, focus, playhead) else { return false };
            let info = info_of(engine, &job);
            let result = job.run(&AtomicBool::new(false));
            self.finish(engine, &info, result)
        }
    }

    /// Installs or records a finished job's result.
    fn finish(&mut self, engine: &mut Engine, info: &Info, result: std::result::Result<VocalDone, JobError>) -> bool {
        if engine.loads() != info.loads {
            return false;
        }
        match result {
            Ok(done) => {
                // A result no clip uses right now is just dropped, never marked failed: an Undo
                // (of a Clean Vocal amount drag, say) may need that analysis again.
                let ok = vocal::install(engine, done);
                if ok {
                    self.message = None;
                }
                ok
            }
            Err(JobError::Cancelled) => false,
            Err(err) => {
                let why = err.to_string();
                vocal::mark_failed_because(engine, info.key, &why);
                self.message = Some(match info.render {
                    Some(_) => format!("{}: {}", info.track, vocal::render_failure_text(&why)),
                    None => format!("{}: vocal analysis failed ({why})", info.track),
                });
                false
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn collect(&mut self, engine: &mut Engine) -> bool {
        use std::sync::mpsc::TryRecvError;
        let Some(r) = &self.running else { return false };
        // A newer edit (or another session) supersedes the render in flight.
        let stale = engine.loads() != r.info.loads
            || r.info.render.is_some_and(|(clip, inputs)| {
                let s = engine.session();
                s.find_clip(clip).and_then(|(_, c)| vocal::render_inputs(s, c)) != Some(inputs)
            });
        if stale {
            r.cancel.store(true, Ordering::Relaxed);
        }
        let result = match r.rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return false,
            // The job's thread died (a panic): treat it as a failed job.
            Err(TryRecvError::Disconnected) => Err(JobError::Unsupported("the vocal job stopped unexpectedly".into())),
        };
        let Some(r) = self.running.take() else { return false };
        self.finish(engine, &r.info, result)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn start(&mut self, engine: &mut Engine, job: VocalJob) {
        let info = info_of(engine, &job);
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let flag = Arc::clone(&cancel);
        // The job moves into the thread; if no thread can be started it comes back to run inline.
        let slot = Arc::new(std::sync::Mutex::new(Some(job)));
        let theirs = Arc::clone(&slot);
        let spawned = std::thread::Builder::new()
            .name("vocal-job".into())
            .spawn(move || {
                let job = theirs.lock().ok().and_then(|mut j| j.take());
                if let Some(job) = job {
                    let _ = tx.send(job.run(&flag));
                }
            })
            .is_ok();
        if spawned {
            self.running = Some(Running { info, cancel, rx });
        } else if let Some(job) = slot.lock().ok().and_then(|mut j| j.take()) {
            let result = job.run(&cancel);
            self.finish(engine, &info, result);
        }
    }
}
