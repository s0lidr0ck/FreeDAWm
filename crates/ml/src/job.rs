//! Cancellation and progress for long jobs.
//!
//! A job runs on a worker thread. The UI keeps a [`Cancel`] clone and sets it; the job checks it
//! between chunks ([`Ctx::check`]) and stops with [`crate::MlError::Cancelled`]. Progress is
//! reported as a fraction 0..=1 of the whole job; a multi-stage job hands each stage a
//! [`Ctx::stage`] that maps the stage's own 0..=1 onto its slice of the whole.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// A shared cancel flag.
#[derive(Clone, Debug, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the job to stop at its next check.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// A progress value shared between a worker and the UI (an `f32` fraction in an atomic), with a
/// short line saying what the job is doing.
#[derive(Clone, Debug, Default)]
pub struct ProgressCell {
    bits: Arc<AtomicU32>,
    what: Arc<std::sync::Mutex<String>>,
}

impl ProgressCell {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, f: f32) {
        let f = if f.is_finite() { f.clamp(0.0, 1.0) } else { 0.0 };
        self.bits.store(f.to_bits(), Ordering::Relaxed);
    }

    pub fn get(&self) -> f32 {
        f32::from_bits(self.bits.load(Ordering::Relaxed))
    }

    pub fn set_what(&self, s: &str) {
        if let Ok(mut w) = self.what.lock() {
            s.clone_into(&mut w);
        }
    }

    pub fn what(&self) -> String {
        self.what.lock().map(|w| w.clone()).unwrap_or_default()
    }
}

/// The context a job runs in: its cancel flag and where its progress goes.
#[derive(Clone)]
pub struct Ctx {
    cancel: Cancel,
    sink: Arc<dyn Fn(f32, &str) + Send + Sync>,
    lo: f32,
    hi: f32,
}

impl std::fmt::Debug for Ctx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ctx").field("cancelled", &self.cancel.is_cancelled()).field("lo", &self.lo).field("hi", &self.hi).finish()
    }
}

impl Default for Ctx {
    fn default() -> Self {
        Self::new(Cancel::new(), |_, _| {})
    }
}

impl Ctx {
    /// A context reporting the whole job's progress to `sink(fraction, what)`.
    pub fn new(cancel: Cancel, sink: impl Fn(f32, &str) + Send + Sync + 'static) -> Self {
        Ctx { cancel, sink: Arc::new(sink), lo: 0.0, hi: 1.0 }
    }

    /// A context reporting into a [`ProgressCell`].
    pub fn with_cell(cancel: Cancel, cell: ProgressCell) -> Self {
        Self::new(cancel, move |f, what| {
            cell.set(f);
            if !what.is_empty() {
                cell.set_what(what);
            }
        })
    }

    pub fn cancel_flag(&self) -> &Cancel {
        &self.cancel
    }

    /// `Err(Cancelled)` once the job was asked to stop.
    pub fn check(&self) -> crate::Result<()> {
        if self.cancel.is_cancelled() { Err(crate::MlError::Cancelled) } else { Ok(()) }
    }

    /// Report this stage's progress `f` (0..=1).
    pub fn report(&self, f: f32) {
        let f = if f.is_finite() { f.clamp(0.0, 1.0) } else { 0.0 };
        (self.sink)(self.lo + (self.hi - self.lo) * f, "");
    }

    /// Say what the job is doing now (shown under the progress bar).
    pub fn say(&self, what: &str) {
        (self.sink)(self.lo, what);
    }

    /// The sub-context for the part `a..b` (fractions of this stage).
    pub fn stage(&self, a: f32, b: f32) -> Ctx {
        let span = self.hi - self.lo;
        let (a, b) = (a.clamp(0.0, 1.0), b.clamp(0.0, 1.0));
        Ctx { cancel: self.cancel.clone(), sink: Arc::clone(&self.sink), lo: self.lo + span * a, hi: self.lo + span * b.max(a) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_map_into_the_whole() {
        let cell = ProgressCell::new();
        let ctx = Ctx::with_cell(Cancel::new(), cell.clone());
        let s = ctx.stage(0.5, 1.0).stage(0.5, 1.0);
        s.report(0.0);
        assert!((cell.get() - 0.75).abs() < 1e-6);
        s.report(1.0);
        assert!((cell.get() - 1.0).abs() < 1e-6);
        s.report(f32::NAN);
        assert!((cell.get() - 0.75).abs() < 1e-6);
        assert!(ctx.check().is_ok());
        ctx.cancel_flag().cancel();
        assert_eq!(s.check(), Err(crate::MlError::Cancelled));
    }
}
