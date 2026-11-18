//! Realtime scheduling for the audio thread and the realtime worker pool.
//!
//! The audio device's thread asks for realtime priority once, at its first callback
//! ([`promote_audio_thread`]); the strips it renders in parallel go to a pool of its own
//! ([`start_workers`], made on the control thread before the device starts) whose threads ask
//! for realtime priority as they start. Offline renders (bounces, Process, freezes) keep the
//! ordinary pool at ordinary priority, so a long bounce never starves the desktop.
//!
//! Nothing here can stop audio: priority that is refused (no `rtprio` limit on Linux, no
//! privileges) leaves the threads as they were, and [`status`] says so for the System Usage
//! window. Where the audio system's own thread is realtime already (Core Audio, JACK, PipeWire)
//! it is left alone. Not in the browser build: there [`status`] says priority is not used.

use std::sync::atomic::{AtomicU8, Ordering};

/// What a thread (or the worker pool) got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Priority {
    /// Not asked yet (no device started, or no callback yet).
    NotTried,
    /// Realtime priority, given to FreeDAWm on request.
    On,
    /// The audio system's own thread runs at realtime priority already (left as it is).
    ByAudioSystem,
    /// Asked and refused: the thread runs at ordinary priority (audio still plays).
    NotGranted,
    /// Not used on this target (the browser build).
    Unavailable,
}

impl Priority {
    fn from_u8(v: u8) -> Priority {
        match v {
            1 => Priority::On,
            2 => Priority::ByAudioSystem,
            3 => Priority::NotGranted,
            4 => Priority::Unavailable,
            _ => Priority::NotTried,
        }
    }

    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn to_u8(self) -> u8 {
        match self {
            Priority::NotTried => 0,
            Priority::On => 1,
            Priority::ByAudioSystem => 2,
            Priority::NotGranted => 3,
            Priority::Unavailable => 4,
        }
    }

    /// A short word for the user: "on", "on (by the audio system)", "not granted"...
    pub fn word(self) -> &'static str {
        match self {
            Priority::NotTried => "not started",
            Priority::On => "on",
            Priority::ByAudioSystem => "on (set by the audio system)",
            Priority::NotGranted => "not granted",
            Priority::Unavailable => "not used here",
        }
    }
}

/// The audio thread's and the worker pool's priority, for the System Usage window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RtStatus {
    pub audio: Priority,
    pub workers: Priority,
    /// Workers that got realtime priority, of how many.
    pub workers_on: usize,
    pub workers_total: usize,
}

impl RtStatus {
    /// "Realtime priority: on (audio thread on; 3 of 3 workers on)" and the like.
    pub fn label(&self) -> String {
        if self.audio == Priority::Unavailable {
            return "Realtime priority: not used in the browser".to_string();
        }
        let workers = match self.workers {
            Priority::On => format!("{} of {} workers on", self.workers_on, self.workers_total),
            w => format!("workers {}", w.word()),
        };
        let overall = match (self.audio, self.workers) {
            (Priority::On | Priority::ByAudioSystem, Priority::On) => "on",
            (Priority::NotTried, _) => "not started",
            (Priority::NotGranted, Priority::NotGranted | Priority::NotTried) => "not granted",
            _ => "partly on",
        };
        format!("Realtime priority: {overall} (audio thread {}; {workers})", self.audio.word())
    }

    /// What to do when priority was refused, or `None` when there is nothing to do.
    pub fn hint(&self) -> Option<&'static str> {
        (self.audio == Priority::NotGranted || self.workers == Priority::NotGranted).then_some(if cfg!(target_os = "linux") {
            "Audio plays at ordinary priority and may drop out under load. Allow realtime scheduling \
             (an rtprio limit for your user, e.g. the audio group) and restart FreeDAWm."
        } else {
            "Audio plays at ordinary priority and may drop out under load."
        })
    }
}

static AUDIO: AtomicU8 = AtomicU8::new(0);
static WORKERS: AtomicU8 = AtomicU8::new(0);
static WORKERS_ON: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static WORKERS_TOTAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The audio thread's and the worker pool's priority now.
pub fn status() -> RtStatus {
    if cfg!(target_arch = "wasm32") {
        return RtStatus { audio: Priority::Unavailable, workers: Priority::Unavailable, workers_on: 0, workers_total: 0 };
    }
    RtStatus {
        audio: Priority::from_u8(AUDIO.load(Ordering::Relaxed)),
        workers: Priority::from_u8(WORKERS.load(Ordering::Relaxed)),
        workers_on: WORKERS_ON.load(Ordering::Relaxed),
        workers_total: WORKERS_TOTAL.load(Ordering::Relaxed),
    }
}

/// What a thread gets: left alone when the audio system made it realtime already, else what
/// `ask` gets it (`true`: realtime granted).
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn decide(already_realtime: bool, ask: impl FnOnce() -> bool) -> Priority {
    if already_realtime {
        Priority::ByAudioSystem
    } else if ask() {
        Priority::On
    } else {
        Priority::NotGranted
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod os {
    /// The audio thread's priority (1–99, Linux SCHED_FIFO; 99 is TimeCritical on Windows). Below
    /// JACK's and PipeWire's own (88 and above) and the kernel's IRQ threads (50 is their default,
    /// so realtime kernels keep the sound card's interrupt first: 45 here).
    pub const AUDIO: u8 = 45;
    /// The realtime workers: one below the audio thread, which waits on them.
    pub const WORKER: u8 = 44;

    /// Whether this thread runs at realtime priority already (the audio system made it so).
    pub fn is_realtime() -> bool {
        #[cfg(target_os = "macos")]
        {
            // Core Audio's IO thread is a time-constraint thread, which pthread does not report:
            // it is always realtime, and must not be changed.
            true
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            matches!(thread_priority::unix::thread_schedule_policy(), Ok(thread_priority::unix::ThreadSchedulePolicy::Realtime(_)))
        }
        #[cfg(not(unix))]
        {
            false
        }
    }

    /// Asks for realtime priority `p` for this thread; false when refused (nothing changed).
    pub fn ask(p: u8) -> bool {
        #[cfg(unix)]
        {
            use thread_priority::unix::{RealtimeThreadSchedulePolicy, ThreadSchedulePolicy, set_thread_priority_and_policy, thread_native_id};
            use thread_priority::{ThreadPriority, ThreadPriorityValue};
            let policy = ThreadSchedulePolicy::Realtime(RealtimeThreadSchedulePolicy::Fifo);
            // The policy's own range (1–99 on Linux, narrower on macOS and the BSDs).
            let (lo, hi) = match (ThreadPriority::min_value_for_policy(policy), ThreadPriority::max_value_for_policy(policy)) {
                (Ok(lo), Ok(hi)) => (lo.min(hi), lo.max(hi)),
                _ => return false,
            };
            let want = i32::from(p).clamp(lo, hi);
            let Some(value) = u8::try_from(want).ok().and_then(|v| ThreadPriorityValue::try_from(v).ok()) else {
                return false;
            };
            set_thread_priority_and_policy(thread_native_id(), ThreadPriority::Crossplatform(value), policy).is_ok()
        }
        #[cfg(windows)]
        {
            use thread_priority::{ThreadPriority, ThreadPriorityValue, set_current_thread_priority};
            // 99 is TimeCritical, 80–98 Highest (what the workers get).
            let p = if p >= AUDIO { 99 } else { 85 };
            ThreadPriorityValue::try_from(p).is_ok_and(|v| set_current_thread_priority(ThreadPriority::Crossplatform(v)).is_ok())
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = p;
            false
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static ASKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Called by the audio device's callback each time it runs: the first time on a thread, asks for
/// realtime priority for it (one system call; nothing allocated or logged), after that it only
/// reads a thread-local flag. Never fails: refused priority is shown in [`status`].
pub fn promote_audio_thread() {
    #[cfg(not(target_arch = "wasm32"))]
    if !ASKED.replace(true) {
        let got = decide(os::is_realtime(), || os::ask(os::AUDIO));
        AUDIO.store(got.to_u8(), Ordering::Relaxed);
    }
}

/// [`promote_audio_thread`] for an input device's own thread (recording through a second
/// device): asked for the same way, not shown in [`status`] (which follows the output's).
pub fn promote_capture_thread() {
    #[cfg(not(target_arch = "wasm32"))]
    if !ASKED.replace(true) {
        let _ = decide(os::is_realtime(), || os::ask(os::AUDIO));
    }
}

/// The realtime worker pool, once [`start_workers`] made one whose workers got priority.
#[cfg(not(target_arch = "wasm32"))]
static POOL: std::sync::OnceLock<Option<rayon::ThreadPool>> = std::sync::OnceLock::new();

/// The realtime worker pool for strips rendered for the audio thread, if there is one.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn pool() -> Option<&'static rayon::ThreadPool> {
    POOL.get().and_then(Option::as_ref)
}

/// Makes `threads` workers named `freedawm-rt-N`, each asking `ask` for priority as it starts;
/// the pool, how many got it. No pool when none did (the ordinary pool then serves as before).
#[cfg(not(target_arch = "wasm32"))]
fn build_pool(threads: usize, ask: fn() -> bool) -> (Option<rayon::ThreadPool>, usize) {
    let Ok(pool) = rayon::ThreadPoolBuilder::new().num_threads(threads).thread_name(|i| format!("freedawm-rt-{i}")).build() else {
        return (None, 0);
    };
    // Every worker asks once, on itself; the caller waits for all of them.
    let on = pool.broadcast(|_| ask()).into_iter().filter(|&ok| ok).count();
    ((on > 0).then_some(pool), on)
}

/// Makes the realtime worker pool (once per run; later calls do nothing). Call on the control
/// thread before the audio device starts: it starts threads and waits for them.
pub fn start_workers() {
    #[cfg(not(target_arch = "wasm32"))]
    start_workers_with(|| os::ask(os::WORKER));
}

#[cfg(not(target_arch = "wasm32"))]
fn start_workers_with(ask: fn() -> bool) {
    POOL.get_or_init(|| {
        // As many as the ordinary pool has, less the audio thread's own core.
        let threads = rayon::current_num_threads().saturating_sub(1).max(1);
        let (pool, on) = build_pool(threads, ask);
        WORKERS_TOTAL.store(threads, Ordering::Relaxed);
        WORKERS_ON.store(on, Ordering::Relaxed);
        WORKERS.store(if on > 0 { Priority::On } else { Priority::NotGranted }.to_u8(), Ordering::Relaxed);
        pool
    });
}

/// Tests: makes the realtime pool with workers that report priority without asking the system
/// (so a test never runs threads at realtime priority).
#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) fn start_test_workers() {
    start_workers_with(|| true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refused_priority_is_reported_and_nothing_else_happens() {
        assert_eq!(decide(false, || false), Priority::NotGranted);
        assert_eq!(decide(false, || true), Priority::On);
        let mut asked = false;
        assert_eq!(
            decide(true, || {
                asked = true;
                true
            }),
            Priority::ByAudioSystem
        );
        assert!(!asked, "a thread the audio system made realtime is left alone");
    }

    #[test]
    fn the_label_says_what_was_granted() {
        let s = |audio, workers, on| RtStatus { audio, workers, workers_on: on, workers_total: 3 };
        assert_eq!(s(Priority::On, Priority::On, 3).label(), "Realtime priority: on (audio thread on; 3 of 3 workers on)");
        let refused = s(Priority::NotGranted, Priority::NotGranted, 0);
        assert_eq!(refused.label(), "Realtime priority: not granted (audio thread not granted; workers not granted)");
        assert!(refused.hint().is_some());
        let core_audio = s(Priority::ByAudioSystem, Priority::NotGranted, 0);
        assert!(core_audio.label().starts_with("Realtime priority: partly on"), "{}", core_audio.label());
        assert_eq!(s(Priority::NotTried, Priority::NotTried, 0).label().split(' ').nth(2), Some("not"));
        assert!(s(Priority::On, Priority::On, 3).hint().is_none());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_pool_whose_workers_were_all_refused_is_not_used() {
        let (pool, on) = build_pool(2, || false);
        assert!(pool.is_none() && on == 0);
        let (pool, on) = build_pool(2, || true);
        assert!(pool.is_some() && on == 2);
    }

    /// The real request on a thread of its own: granted or refused, it returns, and the thread
    /// runs on (the fallback path on a machine without realtime rights, CI's usual case).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn asking_for_realtime_never_fails_the_thread() {
        let got = std::thread::spawn(|| {
            promote_audio_thread();
            let first = status().audio;
            promote_audio_thread();
            (first, status().audio, (0..1000u32).sum::<u32>())
        })
        .join()
        .unwrap();
        assert!(matches!(got.0, Priority::On | Priority::NotGranted | Priority::ByAudioSystem), "{:?}", got.0);
        assert_eq!(got.0, got.1, "asked once per thread");
        assert_eq!(got.2, 499_500);
    }
}
