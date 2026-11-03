//! Audio-to-UI sample streams: a lock-free single-producer ring a plugin pushes its mono input
//! and output into, read by the UI for spectrum and scope displays.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

/// Samples kept per ring.
pub const RING_LEN: usize = 8192;

/// A fixed-capacity ring of the newest `RING_LEN` samples. One writer; any number of readers.
/// A reader may see a torn mix of old and new samples, which is fine for a display.
pub struct AnalysisRing {
    buf: Box<[AtomicU32]>,
    /// Total samples ever written (the next write index, unwrapped).
    write: AtomicUsize,
}

impl Default for AnalysisRing {
    fn default() -> Self {
        Self::new()
    }
}

impl AnalysisRing {
    /// Allocates the ring (call outside the audio thread).
    pub fn new() -> Self {
        AnalysisRing { buf: (0..RING_LEN).map(|_| AtomicU32::new(0)).collect(), write: AtomicUsize::new(0) }
    }

    /// Appends a sample; non-finite values are stored as 0.
    pub fn push(&self, x: f32) {
        let w = self.write.load(Ordering::Relaxed);
        self.store(w, x);
        self.write.store(w.wrapping_add(1), Ordering::Release);
    }

    /// Appends a block of samples.
    pub fn push_slice(&self, xs: &[f32]) {
        let mut w = self.write.load(Ordering::Relaxed);
        for &x in xs {
            self.store(w, x);
            w = w.wrapping_add(1);
        }
        self.write.store(w, Ordering::Release);
    }

    fn store(&self, w: usize, x: f32) {
        let x = if x.is_finite() { x } else { 0.0 };
        if let Some(c) = self.buf.get(w % RING_LEN) {
            c.store(x.to_bits(), Ordering::Relaxed);
        }
    }

    /// Copies the newest `out.len()` samples, oldest first, into `out`. Returns how many were
    /// valid (fewer than asked before that many were written, or when asking for more than
    /// `RING_LEN`); the valid samples fill the end of `out`'s requested span, the rest is
    /// zeroed.
    pub fn latest(&self, out: &mut [f32]) -> usize {
        let w = self.write.load(Ordering::Acquire);
        let want = out.len();
        let valid = want.min(RING_LEN).min(w);
        let pad = want - valid;
        for (i, o) in out.iter_mut().enumerate() {
            *o = if i < pad {
                0.0
            } else {
                let idx = w.wrapping_sub(want - i) % RING_LEN;
                self.buf.get(idx).map_or(0.0, |c| f32::from_bits(c.load(Ordering::Relaxed)))
            };
        }
        valid
    }

    /// Total samples ever written.
    pub fn written(&self) -> usize {
        self.write.load(Ordering::Acquire)
    }
}

/// A fixed array of live values a plugin publishes after each block for its editor to draw (CleanQ:
/// each band's dynamic offset and the dynamic ceiling's cut). Allocated once, when the plugin is
/// built; the audio thread only stores into it (atomic, no allocation). What each index means is
/// the plugin's business.
#[derive(Default)]
pub struct LiveValues {
    values: Box<[AtomicU32]>,
}

impl LiveValues {
    /// `n` values, all 0 (call outside the audio thread).
    pub fn new(n: usize) -> Self {
        LiveValues { values: (0..n).map(|_| AtomicU32::new(0)).collect() }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Stores value `i` (audio thread). Non-finite `v` and out of range: ignored, so an editor
    /// never reads a NaN meter.
    pub fn set(&self, i: usize, v: f32) {
        if !v.is_finite() {
            return;
        }
        if let Some(w) = self.values.get(i) {
            w.store(v.to_bits(), Ordering::Relaxed);
        }
    }

    /// Raises value `i` to `v` if `v` is larger (audio thread; a lock-free compare-and-swap, as
    /// the C++ `raiseMax` on an atomic). Non-finite `v` and out of range: ignored.
    pub fn raise_max(&self, i: usize, v: f32) {
        let Some(w) = self.values.get(i) else { return };
        if !v.is_finite() {
            return;
        }
        let _ = w.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |b| (v > f32::from_bits(b)).then_some(v.to_bits()));
    }

    /// Value `i`, reset to 0 in the same step (the C++ editor's `exchange (0)`). Out of range: NaN.
    pub fn take(&self, i: usize) -> f32 {
        self.values.get(i).map_or(f32::NAN, |w| f32::from_bits(w.swap(0f32.to_bits(), Ordering::Relaxed)))
    }

    /// Where the values live (tests check it never moves).
    #[doc(hidden)]
    pub fn buffer_address(&self) -> usize {
        self.values.as_ptr() as usize
    }

    /// Value `i` as last stored. Out of range: NaN.
    pub fn get(&self, i: usize) -> f32 {
        self.values.get(i).map_or(f32::NAN, |w| f32::from_bits(w.load(Ordering::Relaxed)))
    }
}

/// Values in an [`Analysis::report`].
pub const REPORT_LEN: usize = 9;

/// The two streams a plugin exposes: the mono sum `(L+R)/2` of its input and of its output, and
/// a small report of the plugin's own (beyond the eight `readouts`, for a reading the UI shows
/// but the meters do not carry, such as Shale's phase measurement): what each value means is the
/// plugin's business.
#[derive(Default)]
pub struct Analysis {
    pub pre: AnalysisRing,
    pub post: AnalysisRing,
    report: [AtomicU32; REPORT_LEN],
    /// The plugin's live values for its editor (empty unless built [`Analysis::with_live`]).
    pub live: LiveValues,
}

impl std::fmt::Debug for Analysis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Analysis").field("pre_written", &self.pre.written()).field("post_written", &self.post.written()).finish()
    }
}

/// Two handles are equal when they are the same streams (identity, not content).
impl PartialEq for Analysis {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl Analysis {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Streams plus `n` [`LiveValues`].
    pub fn with_live(n: usize) -> Analysis {
        Analysis { live: LiveValues::new(n), ..Self::default() }
    }

    /// Writes the report (audio thread; atomic stores only). Values past [`REPORT_LEN`] are
    /// dropped. A reader may see a mix of two reports, which is fine for a display.
    pub fn set_report(&self, values: &[f32]) {
        for (w, v) in self.report.iter().zip(values) {
            w.store(v.to_bits(), Ordering::Relaxed);
        }
    }

    /// The report as last written (all 0 before any).
    pub fn report(&self) -> [f32; REPORT_LEN] {
        let mut out = [0.0; REPORT_LEN];
        for (o, w) in out.iter_mut().zip(&self.report) {
            *o = f32::from_bits(w.load(Ordering::Relaxed));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_returns_latest_in_order_and_wraps() {
        let r = AnalysisRing::new();
        for i in 0..(RING_LEN + 10) {
            r.push(i as f32);
        }
        let mut out = [0.0f32; 4];
        assert_eq!(r.latest(&mut out), 4);
        assert_eq!(out, [(RING_LEN + 6) as f32, (RING_LEN + 7) as f32, (RING_LEN + 8) as f32, (RING_LEN + 9) as f32]);
        r.push(f32::NAN);
        r.latest(&mut out[..1]);
        assert_eq!(out[0], 0.0);
    }

    #[test]
    fn the_report_round_trips_and_ignores_extra_values() {
        let a = Analysis::new();
        assert_eq!(a.report(), [0.0; REPORT_LEN]);
        a.set_report(&[1.0, -2.5, f32::NAN]);
        let r = a.report();
        assert_eq!((r[0], r[1], r[3]), (1.0, -2.5, 0.0));
        assert!(r[2].is_nan());
        a.set_report(&[7.0; 20]);
        assert_eq!(a.report(), [7.0; REPORT_LEN]);
    }

    #[test]
    fn live_values_raise_max_and_take() {
        let l = LiveValues::new(2);
        l.raise_max(0, 3.0);
        l.raise_max(0, 1.0);
        l.raise_max(0, f32::NAN);
        l.raise_max(9, 5.0);
        assert_eq!(l.take(0), 3.0);
        assert_eq!(l.get(0), 0.0, "taken: back to 0");
        assert!(l.take(9).is_nan());
    }

    #[test]
    fn live_values_get_set_and_bounds() {
        let l = LiveValues::new(4);
        assert_eq!(l.len(), 4);
        l.set(2, -3.5);
        assert_eq!(l.get(2), -3.5);
        assert_eq!(l.get(0), 0.0);
        assert!(l.get(9).is_nan());
        l.set(9, 1.0);
        assert!(l.get(9).is_nan());
        assert!(Analysis::default().live.is_empty());
        assert_eq!(Analysis::with_live(281).live.len(), 281);
    }

    #[test]
    fn short_history_and_slices() {
        let r = AnalysisRing::new();
        r.push_slice(&[1.0, 2.0, f32::INFINITY]);
        assert_eq!(r.written(), 3);
        let mut out = [9.0f32; 5];
        assert_eq!(r.latest(&mut out), 3);
        assert_eq!(out, [0.0, 0.0, 1.0, 2.0, 0.0]);
        let mut big = vec![0.0f32; RING_LEN + 5];
        assert_eq!(r.latest(&mut big), 3);
    }
}
