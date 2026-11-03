//! A background worker for curves too expensive to compute on the UI thread (Vint's measured
//! frequency response runs two whole engines). Requests are keyed (one graph each); a new request
//! for a key replaces one still waiting (latest request wins), and the UI picks finished curves up
//! when it next draws. Without threads (wasm32), or when no thread can be started, it computes
//! inline when asked.

use egui::Id;
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

/// What a curve is computed from: (values, sample rate, frequencies) to dB at those frequencies.
pub type CurveFn = dyn Fn(&[f32], f32, &[f32]) -> Vec<f32> + Send + Sync;

/// One request: the hash of its inputs, and the inputs.
struct Job {
    hash: u64,
    values: Vec<f32>,
    sr: f32,
    freqs: Vec<f32>,
}

#[derive(Default)]
struct State {
    /// Waiting requests, one per key (the latest).
    jobs: HashMap<Id, Job>,
    /// Finished curves not yet picked up, one per key (the latest).
    done: HashMap<Id, (u64, Arc<Vec<f32>>)>,
    quit: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    compute: Box<CurveFn>,
    /// Called when a curve is finished (a repaint, so the UI picks it up).
    notify: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The curve of `job`, made safe to draw: one finite value per frequency.
    fn run(&self, job: &Job) -> Arc<Vec<f32>> {
        let mut c = (self.compute)(&job.values, job.sr, &job.freqs);
        c.resize(job.freqs.len(), 0.0);
        c.iter_mut().filter(|v| !v.is_finite()).for_each(|v| *v = 0.0);
        Arc::new(c)
    }
}

/// See the module docs. Dropping it stops its thread (after the curve in hand, if any).
pub struct CurveWorker {
    shared: Arc<Shared>,
    threaded: bool,
}

impl CurveWorker {
    /// A worker computing with `compute` on its own thread (inline on wasm32, or if the thread
    /// cannot be started), calling `notify` whenever a curve is finished.
    pub fn new(compute: Box<CurveFn>, notify: Box<dyn Fn() + Send + Sync>) -> Self {
        let shared = Arc::new(Shared { state: Mutex::new(State::default()), wake: Condvar::new(), compute, notify });
        let threaded = !cfg!(target_arch = "wasm32") && {
            let s = Arc::clone(&shared);
            std::thread::Builder::new().name("curve-worker".into()).spawn(move || work(&s)).is_ok()
        };
        CurveWorker { shared, threaded }
    }

    /// A worker that computes inline, when asked (what wasm32 gets).
    pub fn inline(compute: Box<CurveFn>) -> Self {
        CurveWorker {
            shared: Arc::new(Shared { state: Mutex::new(State::default()), wake: Condvar::new(), compute, notify: Box::new(|| {}) }),
            threaded: false,
        }
    }

    /// Asks for `key`'s curve from these inputs; returns at once (a threaded worker), replacing a
    /// request for `key` still waiting.
    pub fn request(&self, key: Id, hash: u64, values: &[f32], sr: f32, freqs: &[f32]) {
        let job = Job { hash, values: values.to_vec(), sr, freqs: freqs.to_vec() };
        if self.threaded {
            self.shared.lock().jobs.insert(key, job);
            self.shared.wake.notify_one();
        } else {
            let c = self.shared.run(&job);
            self.shared.lock().done.insert(key, (hash, c));
        }
    }

    /// `key`'s latest finished curve (with the hash of its inputs), if one came in since last asked.
    pub fn take(&self, key: Id) -> Option<(u64, Arc<Vec<f32>>)> {
        self.shared.lock().done.remove(&key)
    }
}

impl Drop for CurveWorker {
    fn drop(&mut self) {
        self.shared.lock().quit = true;
        self.shared.wake.notify_all();
    }
}

/// The worker thread: the waiting requests one at a time, until told to quit.
fn work(s: &Shared) {
    loop {
        let (key, job) = {
            let mut st = s.lock();
            loop {
                if st.quit {
                    return;
                }
                let next = st.jobs.keys().next().copied();
                if let Some(job) = next.and_then(|k| st.jobs.remove(&k).map(|j| (k, j))) {
                    break job;
                }
                st = s.wake.wait(st).unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };
        let c = s.run(&job);
        s.lock().done.insert(key, (job.hash, c));
        (s.notify)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// Polls `take` until something arrives (or 5 s pass).
    fn wait_for(w: &CurveWorker, key: Id) -> Option<(u64, Arc<Vec<f32>>)> {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(5) {
            if let Some(d) = w.take(key) {
                return Some(d);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        None
    }

    #[test]
    fn a_request_returns_at_once_and_the_curve_arrives_later() {
        // The compute waits for the test's go-ahead, so a request that blocked would hang here.
        let (go, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let (told, notified) = mpsc::channel::<()>();
        let told = Mutex::new(told);
        let w = CurveWorker::new(
            Box::new(move |v, _, f| {
                let _ = gate.lock().map(|g| g.recv_timeout(Duration::from_secs(5)));
                vec![v[0]; f.len()]
            }),
            Box::new(move || {
                let _ = told.lock().map(|t| t.send(()));
            }),
        );
        let key = Id::new("g");
        w.request(key, 1, &[1.0], 48_000.0, &[100.0, 1000.0]);
        // No clock: the compute cannot finish before the go-ahead below, so a request that waited
        // for it would only return once the gate timed out, with the curve already in.
        assert!(w.take(key).is_none(), "the request returned before the compute finished");
        go.send(()).ok();
        let (h, c) = wait_for(&w, key).expect("the curve arrives");
        assert_eq!((h, c.as_slice()), (1, &[1.0, 1.0][..]));
        assert!(notified.recv_timeout(Duration::from_secs(5)).is_ok(), "the UI is told");
        assert!(w.take(key).is_none(), "taken once");
    }

    #[test]
    fn the_latest_request_wins() {
        let (go, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let (began, started) = mpsc::channel::<()>();
        let began = Mutex::new(began);
        let w = CurveWorker::new(
            Box::new(move |v, _, f| {
                let _ = began.lock().map(|b| b.send(()));
                let _ = gate.lock().map(|g| g.recv_timeout(Duration::from_secs(5)));
                vec![v[0]; f.len()]
            }),
            Box::new(|| {}),
        );
        let key = Id::new("g");
        // The first is picked up and held; the next two wait, and only the last of them is run.
        w.request(key, 1, &[1.0], 48_000.0, &[100.0]);
        assert!(started.recv_timeout(Duration::from_secs(5)).is_ok(), "the first compute began");
        w.request(key, 2, &[2.0], 48_000.0, &[100.0]);
        w.request(key, 3, &[f32::NAN], 48_000.0, &[100.0]);
        for _ in 0..3 {
            go.send(()).ok();
        }
        let mut seen = Vec::new();
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(5) && seen.last().map(|(h, _)| *h) != Some(3) {
            if let Some((h, c)) = w.take(key) {
                seen.push((h, c[0]));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(seen.iter().all(|(h, _)| *h != 2), "the replaced request never ran: {seen:?}");
        assert_eq!(seen.last(), Some(&(3, 0.0)), "the latest arrives, its NaN made safe");
    }

    #[test]
    fn an_inline_worker_computes_when_asked() {
        let w = CurveWorker::inline(Box::new(|v, _, f| vec![v[0]; f.len() + 3]));
        w.request(Id::new("k"), 9, &[4.0], 48_000.0, &[1.0, 2.0]);
        assert_eq!(w.take(Id::new("k")).map(|(h, c)| (h, c.to_vec())), Some((9, vec![4.0, 4.0])));
    }
}
