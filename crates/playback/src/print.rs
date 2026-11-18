//! Real-time bounce: the live player prints the main mix (or one bus) over a range as it plays it,
//! so hardware inserts and sends to outboard gear are heard and printed as they sound.
//!
//! The audio thread copies each block of the source, taken after the master's chain and before
//! the click and the monitor stage, into a lock-free ring ([`PrintTap`]); positions are the
//! timeline's (the block's render position less the mix's delay compensation), so the print starts
//! at the range's first sample whatever the device latency, as an offline bounce does. The UI
//! thread drains the ring ([`PrintTap::drain`]) and, once [`PrintTap::is_done`], hands the audio to
//! the bounce writer.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};

use soundcraft_mix::Tap;
use soundcraft_time::{Range, Samples};

use crate::feed::SampleRing;

/// Seconds of audio the ring holds between two drains.
const RING_SECONDS: usize = 4;

/// A print under way: what the audio thread captures and the UI thread drains.
pub struct PrintTap {
    ring: SampleRing,
    channels: usize,
    bus: Tap,
    range: Range,
    /// The timeline position the next captured frame is (`range.start` until the first).
    next: AtomicI64,
    /// The block holding the range's last frame has been captured.
    done: AtomicBool,
    /// Frames that did not fit the ring or were skipped (the print is then not whole).
    lost: AtomicUsize,
}

impl PrintTap {
    /// A print of `channels` of the main mix (or `bus`: a bus or a hardware output) over `range`,
    /// its ring sized for `rate`.
    pub fn new(range: Range, bus: impl Into<Tap>, channels: usize, rate: u32) -> PrintTap {
        let bus = bus.into();
        let channels = channels.clamp(1, 64);
        let cap = (rate as usize).saturating_mul(channels).saturating_mul(RING_SECONDS);
        PrintTap {
            ring: SampleRing::new(cap),
            channels,
            bus,
            range,
            next: AtomicI64::new(range.start),
            done: AtomicBool::new(range.is_empty()),
            lost: AtomicUsize::new(0),
        }
    }

    pub fn range(&self) -> Range {
        self.range
    }

    /// What is printed: the main mix, a bus or a hardware output.
    pub fn tap(&self) -> Tap {
        self.bus
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Every frame of the range has been captured (it may still wait in the ring).
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    /// Frames lost: the ring overflowed (the UI did not drain it in time) or the transport did not
    /// play the range through. A print with any lost is not the mix as played.
    pub fn lost(&self) -> usize {
        self.lost.load(Ordering::Relaxed)
    }

    /// How far the print is, from 0 to 1.
    pub fn progress(&self) -> f32 {
        let done = self.next.load(Ordering::Relaxed).saturating_sub(self.range.start);
        (done as f64 / self.range.len().max(1) as f64).clamp(0.0, 1.0) as f32
    }

    /// Moves what the ring holds onto the end of `out` (planar, one `Vec` per channel). Off the
    /// audio thread: it may grow `out`.
    pub fn drain(&self, out: &mut Vec<Vec<f32>>) {
        out.resize_with(self.channels, Vec::new);
        let queued = self.ring.len();
        let n = queued - queued % self.channels;
        for o in out.iter_mut() {
            o.reserve(n / self.channels);
        }
        let ch = self.channels;
        self.ring.pop_with(n, |i, x| {
            if let Some(o) = out.get_mut(i % ch) {
                o.push(x);
            }
        });
    }

    /// The audio thread's part: a block of `n` frames of `src` (the source's channels) rendered at
    /// `pos`, which the mix delays by `latency`, so its first frame is the timeline's
    /// `pos - latency`. Frames inside the range are queued; a gap (a jump, a block not played) is
    /// counted lost and filled with silence so the print keeps its timing. Never allocates.
    pub(crate) fn capture(&self, src: &[Vec<f32>], pos: Samples, latency: usize, n: usize) {
        if self.is_done() || n == 0 {
            return;
        }
        let t0 = pos.saturating_sub(i64::try_from(latency).unwrap_or(i64::MAX));
        let t1 = t0.saturating_add(n as i64);
        let next = self.next.load(Ordering::Relaxed);
        let to = t1.min(self.range.end);
        if to <= next {
            return;
        }
        // Silence for what the transport did not play (it started late, or jumped).
        let gap = usize::try_from(t0.max(next).min(to) - next).unwrap_or(0);
        let ch = self.channels;
        if gap > 0 {
            self.lost.fetch_add(gap, Ordering::Relaxed);
            let pushed = self.ring.push(gap * ch, |_| 0.0);
            self.lost.fetch_add(gap - pushed / ch, Ordering::Relaxed);
        }
        let from = t0.max(next);
        let len = usize::try_from(to - from).unwrap_or(0);
        let off = usize::try_from(from - t0).unwrap_or(0);
        if len > 0 {
            let pushed = self.ring.push(len * ch, |i| {
                let (f, c) = (i / ch, i % ch);
                let x = src.get(c).and_then(|b| b.get(off + f)).copied().unwrap_or(0.0);
                if x.is_finite() { x } else { 0.0 }
            });
            // A partial frame would shift the channels: what did not fit is lost whole.
            self.lost.fetch_add(len - pushed / ch, Ordering::Relaxed);
        }
        self.next.store(to, Ordering::Relaxed);
        if to >= self.range.end {
            self.done.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(n: usize, base: f32) -> Vec<Vec<f32>> {
        vec![(0..n).map(|i| base + i as f32).collect(), (0..n).map(|i| -(base + i as f32)).collect()]
    }

    #[test]
    fn captures_the_range_less_the_mix_latency() {
        let p = PrintTap::new(Range::new(100, 300), None, 2, 48_000);
        // Blocks of 64 rendered from 90, delayed by 20: the timeline from 70.
        let mut pos = 90;
        while !p.is_done() {
            p.capture(&src(64, (pos - 20) as f32), pos, 20, 64);
            pos += 64;
        }
        let mut out = Vec::new();
        p.drain(&mut out);
        assert_eq!(out[0], (100..300).map(|t| t as f32).collect::<Vec<_>>());
        assert_eq!(out[1], (100..300).map(|t| -(t as f32)).collect::<Vec<_>>());
        assert_eq!(p.lost(), 0);
        assert!((p.progress() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_jump_is_silence_and_counted_lost() {
        let p = PrintTap::new(Range::new(0, 100), None, 1, 48_000);
        p.capture(&[vec![1.0; 40]], 0, 0, 40);
        // The transport jumped from 40 to 60.
        p.capture(&[vec![2.0; 40]], 60, 0, 40);
        assert_eq!(p.lost(), 20);
        assert!(p.is_done());
        let mut out = Vec::new();
        p.drain(&mut out);
        assert_eq!(out[0].len(), 100);
        assert_eq!(&out[0][38..42], &[1.0, 1.0, 0.0, 0.0]);
        assert_eq!(&out[0][58..62], &[0.0, 0.0, 2.0, 2.0]);
    }

    #[test]
    fn an_overflow_drops_whole_frames_and_counts_them() {
        // The smallest ring holds 2 samples (rounded up from the request), so one stereo frame.
        let p = PrintTap { ring: SampleRing::new(2), ..PrintTap::new(Range::new(0, 4), None, 2, 0) };
        p.capture(&src(4, 0.0), 0, 0, 4);
        assert_eq!(p.lost(), 3);
        let mut out = Vec::new();
        p.drain(&mut out);
        assert_eq!(out, vec![vec![0.0], vec![-0.0]]);
    }
}
