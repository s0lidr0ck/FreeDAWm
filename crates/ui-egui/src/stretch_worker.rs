//! The stretch renders' background host: while a stretched clip is heard through the real-time
//! stretcher, its offline render is made here, one at a time, on a thread, and put in place when
//! done (the engine's `stretch_jobs`). On wasm32 there are no threads: the engine renders in
//! place there, as it always did.

use soundcraft_engine::Engine;
#[cfg(not(target_arch = "wasm32"))]
use soundcraft_engine::stretch_jobs::{self, Job};

#[cfg(not(target_arch = "wasm32"))]
struct Running {
    job: Job,
    loads: u64,
    rx: std::sync::mpsc::Receiver<Option<Vec<Vec<f32>>>>,
}

/// See the module docs.
#[derive(Default)]
pub struct StretchWorker {
    #[cfg(not(target_arch = "wasm32"))]
    running: Option<Running>,
}

impl StretchWorker {
    /// Whether the host can render in the background (the engine should defer its renders).
    pub fn available() -> bool {
        cfg!(not(target_arch = "wasm32"))
    }

    /// Whether a render is in flight (the host keeps polling while it is).
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

    /// One frame's worth of work: put a finished render in place, then start the next one.
    /// Returns true when the session changed (repaint).
    pub fn poll(&mut self, engine: &mut Engine) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let installed = self.collect(engine);
            if self.running.is_none()
                && !engine.session().live_stretch.is_empty()
                && let Some(job) = stretch_jobs::next_job(engine)
            {
                self.start(engine, job);
            }
            installed
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = engine;
            false
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn collect(&mut self, engine: &mut Engine) -> bool {
        use std::sync::mpsc::TryRecvError;
        let Some(r) = &self.running else { return false };
        let out = match r.rx.try_recv() {
            Ok(out) => out,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => None,
        };
        let Some(r) = self.running.take() else { return false };
        match out {
            Some(out) if engine.loads() == r.loads => stretch_jobs::install(engine, &r.job, out),
            Some(_) => false,
            // The material is gone or the thread died: render in place, so it is not offered
            // again and again.
            None => {
                stretch_jobs::finish_all(engine);
                true
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn start(&mut self, engine: &mut Engine, job: Job) {
        let (tx, rx) = std::sync::mpsc::channel();
        let theirs = job.clone();
        let spawned = std::thread::Builder::new()
            .name("stretch-render".into())
            .spawn(move || {
                let _ = tx.send(theirs.run());
            })
            .is_ok();
        if spawned {
            self.running = Some(Running { job, loads: engine.loads(), rx });
        } else if let Some(out) = job.run() {
            stretch_jobs::install(engine, &job, out);
        }
    }
}
