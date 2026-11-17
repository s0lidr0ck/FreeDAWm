//! The stretch and transpose renders' background host: while a stretched clip is heard through
//! the real-time stretcher, or a transposed one through the live shifter, its offline render is
//! made here, one at a time, on a thread, and put in place when done (the engine's
//! `stretch_jobs`). On wasm32 there are no threads: the engine renders stretches in place there,
//! as it always did, and transposes stay live.

use soundcraft_engine::Engine;
#[cfg(not(target_arch = "wasm32"))]
use soundcraft_engine::stretch_jobs::{self, Job, PitchJob};

#[cfg(not(target_arch = "wasm32"))]
enum Done {
    Stretch(Job, Option<Vec<Vec<f32>>>),
    Pitch(PitchJob, Option<soundcraft_model::PitchRender>),
}

#[cfg(not(target_arch = "wasm32"))]
struct Running {
    loads: u64,
    rx: std::sync::mpsc::Receiver<Done>,
}

/// See the module docs.
#[derive(Default)]
pub struct StretchWorker {
    #[cfg(not(target_arch = "wasm32"))]
    running: Option<Running>,
    /// `(Engine::loads, revision)` when no job was found: an idle worker does not re-scan an
    /// unchanged session every frame.
    #[cfg(not(target_arch = "wasm32"))]
    idle_at: Option<(u64, u64)>,
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

    /// One frame's worth of work: put a finished render in place, then start the next one
    /// (stretches first: they are heard grainier). Returns true when the session changed.
    pub fn poll(&mut self, engine: &mut Engine) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let installed = self.collect(engine);
            let state = (engine.loads(), engine.revision);
            if self.running.is_none() && self.idle_at != Some(state) {
                if let Some(job) = stretch_jobs::next_job(engine) {
                    self.start(engine, move || {
                        let out = job.run();
                        Done::Stretch(job, out)
                    });
                } else if let Some(job) = stretch_jobs::next_pitch_job(engine) {
                    self.start(engine, move || {
                        let out = job.run();
                        Done::Pitch(job, out)
                    });
                } else {
                    self.idle_at = Some(state);
                }
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
    fn finish(engine: &mut Engine, done: Done) -> bool {
        match done {
            Done::Stretch(job, Some(out)) => stretch_jobs::install(engine, &job, out),
            // The material is gone: render what is left in place, so it is not offered again.
            Done::Stretch(_, None) => {
                stretch_jobs::finish_all(engine);
                true
            }
            Done::Pitch(job, render) => stretch_jobs::install_pitch(engine, &job, render),
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn collect(&mut self, engine: &mut Engine) -> bool {
        use std::sync::mpsc::TryRecvError;
        let Some(r) = &self.running else { return false };
        let done = match r.rx.try_recv() {
            Ok(done) => Some(done),
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => None,
        };
        let Some(r) = self.running.take() else { return false };
        self.idle_at = None;
        match done {
            Some(done) if engine.loads() == r.loads => Self::finish(engine, done),
            Some(_) => false,
            // The job's thread died: render stretches in place (transposes stay live).
            None => {
                stretch_jobs::finish_all(engine);
                true
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn start(&mut self, engine: &mut Engine, work: impl FnOnce() -> Done + Send + 'static) {
        let (tx, rx) = std::sync::mpsc::channel();
        // The work moves into the thread; if no thread can be started it comes back to run here.
        let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(work)));
        let theirs = std::sync::Arc::clone(&slot);
        let spawned = std::thread::Builder::new()
            .name("stretch-render".into())
            .spawn(move || {
                let work = theirs.lock().ok().and_then(|mut w| w.take());
                if let Some(work) = work {
                    let _ = tx.send(work());
                }
            })
            .is_ok();
        if spawned {
            self.running = Some(Running { loads: engine.loads(), rx });
        } else if let Some(work) = slot.lock().ok().and_then(|mut w| w.take()) {
            Self::finish(engine, work());
        }
    }
}
