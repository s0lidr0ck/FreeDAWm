//! Gild's background host: makes the made-ahead parts of the gildings (Shift, Grit, Double,
//! Harmony, Swell-in) one at a time off the UI thread, the one nearest the playhead first
//! (`soundcraft_engine::gild`).
//!
//! - `poll` once per frame: installs a finished render, cancels one whose gilding changed since it
//!   started (the next poll makes the newer one), then asks for the next job. While a job is in
//!   flight nothing new is asked for.
//! - A job that fails is marked failed in the engine (not offered again) and says why in the
//!   status bar.

use soundcraft_engine::Engine;
use soundcraft_engine::gild::{self, GildDone, GildJob};
use soundcraft_model::ClipId;
use soundcraft_model::gild::render_key;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct Running {
    clip: ClipId,
    id: u32,
    key: u64,
    label: String,
    loads: u64,
    cancel: Arc<AtomicBool>,
    rx: std::sync::mpsc::Receiver<Result<GildDone, String>>,
}

/// See the module docs.
#[derive(Default)]
pub struct GildWorker {
    running: Option<Running>,
    message: Option<String>,
    /// The session (`Engine::loads`, revision) when `next_job` last found nothing: an idle worker
    /// doesn't scan an unchanged session every frame.
    idle_at: Option<(u64, u64)>,
}

impl GildWorker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Status-bar text: the job running ("Gilding Lead Vox…"), else the last failure.
    pub fn busy(&self) -> Option<String> {
        self.running.as_ref().map(|r| r.label.clone()).or_else(|| self.message.clone())
    }

    pub fn running(&self) -> bool {
        self.running.is_some()
    }

    /// One frame's worth of work. True when it installed something (repaint).
    pub fn poll(&mut self, engine: &mut Engine) -> bool {
        let installed = self.collect(engine);
        if self.running.is_none() {
            let state = (engine.loads(), engine.revision);
            if self.idle_at != Some(state) {
                match gild::next_job(engine) {
                    Some(job) => self.start(engine, job),
                    None => self.idle_at = Some(state),
                }
            }
        }
        installed
    }

    fn collect(&mut self, engine: &mut Engine) -> bool {
        use std::sync::mpsc::TryRecvError;
        let Some(r) = &self.running else { return false };
        let s = engine.session();
        let now = s.find_clip(r.clip).and_then(|(_, c)| c.gild.iter().find(|g| g.id == r.id).and_then(|g| render_key(c, g, s.sample_rate.hz())));
        if engine.loads() != r.loads || now != Some(r.key) {
            r.cancel.store(true, Ordering::Relaxed);
        }
        let result = match r.rx.try_recv() {
            Ok(x) => x,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Err("the Gild render stopped unexpectedly".into()),
        };
        let Some(r) = self.running.take() else { return false };
        if engine.loads() != r.loads {
            return false;
        }
        match result {
            Ok(done) => {
                let ok = gild::install(engine, done);
                if ok {
                    self.message = None;
                }
                ok
            }
            Err(why) if why == "cancelled" || r.cancel.load(Ordering::Relaxed) => false,
            Err(why) => {
                gild::mark_failed(engine, r.key, &why);
                self.message = Some(format!("Gild: {why}"));
                false
            }
        }
    }

    fn start(&mut self, engine: &Engine, job: GildJob) {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let (clip, id, key, label) = (job.clip, job.id, job.key, job.label());
        let flag = Arc::clone(&cancel);
        let slot = Arc::new(std::sync::Mutex::new(Some(job)));
        let theirs = Arc::clone(&slot);
        let spawned = std::thread::Builder::new()
            .name("gild-job".into())
            .spawn(move || {
                let job = theirs.lock().ok().and_then(|mut j| j.take());
                if let Some(job) = job {
                    let _ = tx.send(job.run(&flag));
                }
            })
            .is_ok();
        if spawned {
            self.running = Some(Running { clip, id, key, label, loads: engine.loads(), cancel, rx });
        } else if let Some(job) = slot.lock().ok().and_then(|mut j| j.take()) {
            // No thread: run it here (rare), installing on the next poll.
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = tx.send(job.run(&cancel));
            self.running = Some(Running { clip, id, key, label, loads: engine.loads(), cancel, rx });
        }
    }
}
