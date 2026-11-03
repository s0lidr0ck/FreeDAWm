//! Surviving a graphics device loss (a driver reset or TDR, `DXGI_ERROR_DEVICE_REMOVED`).
//!
//! What eframe 0.36 / egui-wgpu 0.36 / wgpu 30 allow:
//! - wgpu reports the loss: a failed `Present` marks the device lost and calls the device-lost
//!   callback at once (`wgpu_core::Device::lose`), on the UI thread, inside eframe's paint.
//! - egui-wgpu's painter creates its device once and never again; it lives inside eframe's
//!   private runner, so a run cannot swap in a new device. Painting with the lost device panics
//!   in egui-wgpu as soon as a vertex or index buffer must grow ("Failed to create staging
//!   buffer"): that is the crash of 2026-10-30.
//! - But eframe's event loop is kept per thread (`run_and_return`), and `run_native` may run
//!   again on the same thread: a new run makes a new window, wgpu instance, device and painter.
//!
//! So on a loss ([`GpuWatch`]) the app draws nothing more (no buffers to grow), closes its
//! plugin editors, autosaves, and closes the window while the `SoundApp` (session, engine,
//! audio device and its stream) stays alive; `main` then starts a new run with it. The audio
//! thread is never touched. [`Recovery`] decides what follows each run: another run, a
//! restart of the process (the device can't be made again in this one), or giving up after
//! repeated resets. A panic that still escapes a run is caught by `main`; one caused by the
//! loss is recovered the same way, any other autosaves first and goes on as before.
//!
//! `GetDeviceRemovedReason` isn't reachable without `unsafe` (wgpu-hal's raw DX12 device), which
//! the app crate forbids; the log has wgpu's own `Present failed: … (0x887A0005)` line, the
//! device-lost message and the adapter (name, driver, driver version, backend).

use eframe::egui_wgpu::RenderState;
use eframe::wgpu;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Asks for a test loss: `--simulate-device-loss` (after 5 s), `--simulate-device-loss=SECS`, or
/// `--simulate-device-loss=SECS,repeat` (every window loses its device: the give-up path).
pub const SIMULATE_FLAG: &str = "--simulate-device-loss";
/// Given to the process started after a reset this process could not recover from.
pub const RESTARTED_FLAG: &str = "--gpu-restarted";
/// Why the device went, as logged (`--gpu-restarted` runs show it in the status line).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loss {
    pub reason: String,
    pub message: String,
}

/// Watches one wgpu device: its loss, and the errors nobody captured.
#[derive(Default)]
pub struct GpuWatch {
    lost: AtomicBool,
    loss: Mutex<Option<Loss>>,
    adapter: Mutex<String>,
    errors: AtomicU64,
    /// Woken on a loss, so the recovery starts without waiting for input.
    repaint: Mutex<Option<egui::Context>>,
}

impl GpuWatch {
    pub fn new() -> Arc<GpuWatch> {
        Arc::new(GpuWatch::default())
    }

    /// Attach to the device eframe made: log the adapter, hear about its loss, and log the
    /// errors nobody captured instead of panicking (wgpu's default handler panics).
    pub fn watch(self: &Arc<Self>, render: &RenderState, ctx: &egui::Context) {
        *self.repaint.lock().unwrap_or_else(PoisonError::into_inner) = Some(ctx.clone());
        let info = render.adapter.get_info();
        let adapter = format!(
            "{} ({:?}, {:?}; driver {} {}; vendor {:#06x} device {:#06x})",
            info.name, info.backend, info.device_type, info.driver, info.driver_info, info.vendor, info.device
        );
        log::info!("graphics adapter: {adapter}");
        *self.adapter.lock().unwrap_or_else(PoisonError::into_inner) = adapter;
        let watch = Arc::clone(self);
        render.device.set_device_lost_callback(move |reason, message| watch.mark_lost(&format!("{reason:?}"), &message));
        let watch = Arc::clone(self);
        render.device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| watch.note_error(&error.to_string())));
    }

    /// The device is gone (the first report counts). Only sets flags and logs: it runs inside
    /// wgpu, possibly on any thread.
    pub fn mark_lost(&self, reason: &str, message: &str) {
        if self.lost.swap(true, Ordering::SeqCst) {
            return;
        }
        let adapter = self.adapter.lock().unwrap_or_else(PoisonError::into_inner).clone();
        log::error!("graphics device lost ({reason}): {message}; adapter: {adapter}. Recovering: the session is autosaved and the window reopens");
        *self.loss.lock().unwrap_or_else(PoisonError::into_inner) = Some(Loss { reason: reason.into(), message: message.into() });
        if let Some(ctx) = &*self.repaint.lock().unwrap_or_else(PoisonError::into_inner) {
            ctx.request_repaint();
        }
    }

    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    pub fn loss(&self) -> Option<Loss> {
        self.loss.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// An error no error scope caught: logged (the first ten, then every thousandth), never a panic.
    fn note_error(&self, error: &str) {
        let n = self.errors.fetch_add(1, Ordering::Relaxed);
        if n < 10 || n.is_multiple_of(1000) {
            log::error!("graphics error #{}: {error}", n.saturating_add(1));
        }
    }
}

/// A test loss asked for by [`SIMULATE_FLAG`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Simulate {
    /// After the window opens.
    pub after: Duration,
    /// In every window, not only the first.
    pub repeat: bool,
}

/// The test loss `--simulate-device-loss` asks for, if any.
pub fn simulate(args: &[String]) -> Option<Simulate> {
    args.iter().find_map(|a| {
        let rest = a.strip_prefix(SIMULATE_FLAG)?;
        if rest.is_empty() {
            return Some(Simulate { after: Duration::from_secs(5), repeat: false });
        }
        let rest = rest.strip_prefix('=')?;
        let (secs, repeat) = match rest.strip_suffix(",repeat") {
            Some(secs) => (secs, true),
            None => (rest, false),
        };
        let secs: f64 = secs.parse().ok()?;
        (secs.is_finite() && secs >= 0.0).then(|| Simulate { after: Duration::from_secs_f64(secs.min(3600.0)), repeat })
    })
}

/// The arguments for the process started in this one's place: the options kept (not the
/// session files, the demo or a simulated loss), then the session to reopen and
/// [`RESTARTED_FLAG`]. Unsaved changes are in the autosave folder, offered at that launch.
pub fn relaunch_args(original: &[String], session: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let mut args = original.iter();
    while let Some(a) = args.next() {
        if a == "--control" {
            if let Some(port) = args.next() {
                out.extend([a.clone(), port.clone()]);
            }
        } else if a == "--no-audio" || a == "--no-splash" {
            out.push(a.clone());
        }
    }
    out.extend(session.map(str::to_string));
    out.push(RESTARTED_FLAG.into());
    out
}

/// How a run of the event loop ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunEnd {
    /// The user quit.
    Quit,
    /// The graphics device was lost and the window closed for a new one.
    DeviceLost,
    /// The run could not start (no window or device).
    StartFailed,
    /// A panic escaped the run; `gpu` when the device loss caused it.
    Panicked { gpu: bool },
}

/// What `main` does next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Next {
    Exit,
    /// Start a new run (window and device) with the same app, after `delay`.
    Run {
        delay: Duration,
    },
    /// Save, say so, and start a new process in this one's place.
    Relaunch,
    /// The device keeps getting lost: save, say so, and exit.
    GiveUp,
    /// Save and let the panic go on (not a graphics problem).
    ResumePanic,
}

/// In-process recoveries allowed within [`RESET_WINDOW`]; one more gives up.
pub const MAX_RESETS: usize = 3;
pub const RESET_WINDOW: Duration = Duration::from_secs(600);
/// New runs tried while the driver comes back (a TDR takes a few seconds), then a new process.
pub const MAX_START_RETRIES: u32 = 6;

/// The recovery state machine (see the module doc).
#[derive(Default)]
pub struct Recovery {
    resets: Vec<Instant>,
    start_failures: u32,
}

impl Recovery {
    /// A run started (a window and device exist again).
    pub fn started(&mut self) {
        self.start_failures = 0;
    }

    /// Whether a recovery is under way (a run ended for a loss since the last good start).
    pub fn recovering(&self) -> bool {
        !self.resets.is_empty()
    }

    pub fn next(&mut self, end: &RunEnd, now: Instant) -> Next {
        match end {
            RunEnd::Quit => Next::Exit,
            RunEnd::Panicked { gpu: false } => Next::ResumePanic,
            RunEnd::DeviceLost | RunEnd::Panicked { gpu: true } => {
                self.resets.retain(|t| now.saturating_duration_since(*t) < RESET_WINDOW);
                self.resets.push(now);
                if self.resets.len() > MAX_RESETS { Next::GiveUp } else { Next::Run { delay: Duration::from_millis(500) } }
            }
            // The first run failing is the old start-up error; it is returned as it was.
            RunEnd::StartFailed if !self.recovering() => Next::Exit,
            RunEnd::StartFailed => {
                self.start_failures = self.start_failures.saturating_add(1);
                if self.start_failures > MAX_START_RETRIES {
                    Next::Relaunch
                } else {
                    Next::Run { delay: Duration::from_secs(u64::from(self.start_failures.min(4))) }
                }
            }
        }
    }
}

/// Whether a panic that escaped a run comes from painting with a lost device (egui-wgpu's
/// buffer uploads), when the device-lost callback hasn't said so already.
pub fn is_gpu_panic(message: &str) -> bool {
    ["Failed to create staging buffer", "wgpu error", "Device lost", "egui_wgpu", "egui-wgpu"].iter().any(|m| message.contains(m))
}

/// The text of a panic's payload.
pub fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload.downcast_ref::<&str>().map(|s| (*s).to_string()).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_default()
}

/// The message shown before the process restarts or gives up.
pub fn farewell(next: &Next, saved: bool) -> String {
    let saved = if saved { "Your session was saved" } else { "Your session could not be autosaved (see the log)" };
    match next {
        Next::GiveUp => format!(
            "The graphics driver keeps resetting. {saved}; FreeDAWm will close.\n\nRestart the computer or update the graphics driver, then open FreeDAWm: it offers your unsaved work."
        ),
        _ => format!("The graphics driver reset. {saved}; FreeDAWm will restart."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_loss_is_recorded_once_with_its_reason() {
        let w = GpuWatch::new();
        assert!(!w.is_lost() && w.loss().is_none());
        w.mark_lost("Unknown", "Device lost");
        w.mark_lost("Destroyed", "later");
        assert!(w.is_lost());
        assert_eq!(w.loss(), Some(Loss { reason: "Unknown".into(), message: "Device lost".into() }));
        // Errors nobody captured are counted, never a panic.
        for _ in 0..2000 {
            w.note_error("Validation Error");
        }
        assert_eq!(w.errors.load(Ordering::Relaxed), 2000);
    }

    #[test]
    fn a_device_loss_starts_a_new_run_until_it_keeps_happening() {
        let mut r = Recovery::default();
        let t0 = Instant::now();
        assert_eq!(r.next(&RunEnd::Quit, t0), Next::Exit);
        for i in 0..MAX_RESETS {
            let end = if i == 1 { RunEnd::Panicked { gpu: true } } else { RunEnd::DeviceLost };
            assert!(matches!(r.next(&end, t0 + Duration::from_secs(i as u64)), Next::Run { .. }), "reset {i}");
            r.started();
        }
        assert_eq!(r.next(&RunEnd::DeviceLost, t0 + Duration::from_secs(10)), Next::GiveUp);
        // Resets long ago don't count.
        let mut r = Recovery::default();
        for i in 0..10u64 {
            assert!(matches!(r.next(&RunEnd::DeviceLost, t0 + RESET_WINDOW * (i as u32)), Next::Run { .. }));
        }
    }

    #[test]
    fn a_device_that_cant_be_made_again_is_retried_then_the_process_restarts() {
        let mut r = Recovery::default();
        let t0 = Instant::now();
        // At launch, a failed start is the start-up error it always was.
        assert_eq!(r.next(&RunEnd::StartFailed, t0), Next::Exit);
        assert!(matches!(r.next(&RunEnd::DeviceLost, t0), Next::Run { .. }));
        let mut delays = Vec::new();
        for _ in 0..MAX_START_RETRIES {
            match r.next(&RunEnd::StartFailed, t0) {
                Next::Run { delay } => delays.push(delay),
                other => panic!("{other:?}"),
            }
        }
        assert!(delays.windows(2).all(|w| w[0] <= w[1]) && delays.iter().all(|d| *d <= Duration::from_secs(4)));
        assert_eq!(r.next(&RunEnd::StartFailed, t0), Next::Relaunch);
        // A good start resets the count.
        r.started();
        assert!(matches!(r.next(&RunEnd::StartFailed, t0), Next::Run { .. }));
    }

    #[test]
    fn other_panics_go_on_after_saving() {
        assert_eq!(Recovery::default().next(&RunEnd::Panicked { gpu: false }, Instant::now()), Next::ResumePanic);
        assert!(is_gpu_panic("Failed to create staging buffer for index data. Index count: 7905."));
        assert!(!is_gpu_panic("index out of bounds"));
        assert_eq!(panic_text(&"boom"), "boom");
        assert_eq!(panic_text(&String::from("bang")), "bang");
    }

    #[test]
    fn the_restarted_process_reopens_the_session_with_the_options_kept() {
        let original = args(&["--demo", "--control", "0", "--no-audio", "--simulate-device-loss=3", "old.fdawm", "take.wav"]);
        assert_eq!(
            relaunch_args(&original, Some("C:/Songs/Song.fdawm")),
            args(&["--control", "0", "--no-audio", "C:/Songs/Song.fdawm", RESTARTED_FLAG])
        );
        assert_eq!(relaunch_args(&[], None), args(&[RESTARTED_FLAG]));
        // A trailing `--control` without its port is dropped.
        assert_eq!(relaunch_args(&args(&["--control"]), None), args(&[RESTARTED_FLAG]));
    }

    #[test]
    fn the_simulated_loss_flag_takes_seconds() {
        let at = |secs: f64, repeat| Some(Simulate { after: Duration::from_secs_f64(secs), repeat });
        assert_eq!(simulate(&args(&["--demo"])), None);
        assert_eq!(simulate(&args(&[SIMULATE_FLAG])), at(5.0, false));
        assert_eq!(simulate(&args(&["--simulate-device-loss=2.5"])), at(2.5, false));
        assert_eq!(simulate(&args(&["--simulate-device-loss=3,repeat"])), at(3.0, true));
        assert_eq!(simulate(&args(&["--simulate-device-loss=-1"])), None);
        assert_eq!(simulate(&args(&["--simulate-device-loss=NaN"])), None);
        assert_eq!(simulate(&args(&["--simulate-device-loss=3,again"])), None);
        assert_eq!(simulate(&args(&["--simulate-device-lossy"])), None);
    }

    #[test]
    fn the_farewell_says_the_session_was_saved() {
        assert_eq!(farewell(&Next::Relaunch, true), "The graphics driver reset. Your session was saved; FreeDAWm will restart.");
        assert!(farewell(&Next::GiveUp, false).contains("could not be autosaved"));
    }
}
