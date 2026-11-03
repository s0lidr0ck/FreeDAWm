//! The remote mix stream's two ends on the audio thread (spec
//! `docs/superpowers/specs/2026-11-02-remote-audio-recording-design.md`, section 2), both
//! lock-free and allocation-free there; everything else (encoding, the network, the jitter
//! buffer) runs on other threads (the network crate).
//!
//! - [`MixTap`] (the host): while on, each rendered block of the main mix (what the host hears)
//!   is queued with where it sits: the take clock (frames rolled since Play) and the timeline
//!   position of its first frame, and whether the transport rolls. A block that doesn't fit is
//!   dropped whole (the reader sees the gap in the take clock).
//! - [`StreamIn`] (the performer): the decoded stream, at the session rate, is added to the main
//!   mix at a gain, in order; each block notes how many stream frames had played before it and
//!   the take clock at its first frame, so the network side knows which stream frame the
//!   performer heard at which moment of his own take clock.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering, fence};

use crate::feed::SampleRing;

/// Samples each ring holds (interleaved stereo plus block headers): over a second at 96 kHz.
const RING: usize = 1 << 18;
/// A tap block's header: frames, take clock (2 words), timeline (2 words), rolling.
const HEADER: usize = 6;

fn split(v: i64) -> (f32, f32) {
    let u = v as u64;
    (f32::from_bits(u as u32), f32::from_bits((u >> 32) as u32))
}

fn join(lo: f32, hi: f32) -> i64 {
    (u64::from(lo.to_bits()) | (u64::from(hi.to_bits()) << 32)) as i64
}

/// The host's tap on its main mix (see the module docs).
pub struct MixTap {
    on: AtomicBool,
    ring: SampleRing,
    dropped: AtomicU64,
}

/// One block read off a [`MixTap`].
#[derive(Debug, Clone, PartialEq)]
pub struct TapBlock {
    /// Frames rolled since Play at the block's first frame (the take clock), and its timeline
    /// position; `rolling` false while stopped (monitoring only).
    pub rolled: i64,
    pub position: i64,
    pub rolling: bool,
    /// Interleaved stereo.
    pub samples: Vec<f32>,
}

impl Default for MixTap {
    fn default() -> Self {
        MixTap { on: AtomicBool::new(false), ring: SampleRing::new(RING), dropped: AtomicU64::new(0) }
    }
}

impl MixTap {
    /// Starts or stops tapping (the stream is wanted).
    pub fn set_on(&self, on: bool) {
        self.on.store(on, Ordering::Release);
    }

    pub fn is_on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// Blocks dropped because the reader fell behind.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// The audio thread: a rendered block of `frames` (`left`, `right`; mono when they are the
    /// same slice) at take clock `rolled` and timeline `position`. Never allocates or blocks.
    pub fn push_block(&self, left: &[f32], right: &[f32], frames: usize, rolled: i64, position: i64, rolling: bool) {
        if !self.on.load(Ordering::Relaxed) || frames == 0 {
            return;
        }
        let need = HEADER + frames * 2;
        if self.ring.room() < need {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let (r0, r1) = split(rolled);
        let (p0, p1) = split(position);
        let head = [f32::from_bits(frames as u32), r0, r1, p0, p1, if rolling { 1.0 } else { 0.0 }];
        self.ring.push(need, |i| {
            if let Some(h) = head.get(i) {
                return *h;
            }
            let k = i - HEADER;
            let side = if k.is_multiple_of(2) { left } else { right };
            side.get(k / 2).copied().map_or(0.0, crate::feed::finite)
        });
    }

    /// Off the audio thread: the blocks queued.
    pub fn read(&self, out: &mut Vec<TapBlock>) {
        let mut buf = Vec::new();
        // Whole blocks only: a header says how long its block is.
        loop {
            if self.ring.len() < HEADER {
                return;
            }
            buf.clear();
            buf.resize(HEADER, 0.0);
            let frames = self.ring.peek(0).to_bits() as usize;
            if self.ring.len() < HEADER + frames * 2 {
                return;
            }
            self.ring.pop_exact(&mut buf);
            let rolled = join(buf[1], buf[2]);
            let position = join(buf[3], buf[4]);
            let rolling = buf[5] > 0.5;
            let mut samples = vec![0.0; frames * 2];
            self.ring.pop_exact(&mut samples);
            out.push(TapBlock { rolled, position, rolling, samples });
        }
    }

    /// Forgets what is queued (off the audio thread, the reader).
    pub fn clear(&self) {
        self.ring.clear();
    }
}

/// The performer's end of the stream (see the module docs).
pub struct StreamIn {
    on: AtomicBool,
    ring: SampleRing,
    /// Gain × 1000.
    gain: AtomicU32,
    /// Stream frames played so far (silence while it ran dry not counted).
    played: AtomicU64,
    /// Frames of silence played because the stream ran dry.
    starved: AtomicU64,
    /// The last block: stream frames played before it and the take clock at its first frame
    /// (a seqlock: `seq` odd while it is written).
    seq: AtomicU64,
    mark_played: AtomicU64,
    mark_rolled: AtomicI64,
    mark_rolling: AtomicBool,
    mark_full: AtomicBool,
    /// The reader asks the audio thread to drop what is queued (a locate on the host).
    flush: AtomicBool,
}

impl Default for StreamIn {
    fn default() -> Self {
        StreamIn {
            on: AtomicBool::new(false),
            ring: SampleRing::new(RING),
            gain: AtomicU32::new(1000),
            played: AtomicU64::new(0),
            starved: AtomicU64::new(0),
            seq: AtomicU64::new(0),
            mark_played: AtomicU64::new(0),
            mark_rolled: AtomicI64::new(0),
            mark_rolling: AtomicBool::new(false),
            mark_full: AtomicBool::new(false),
            flush: AtomicBool::new(false),
        }
    }
}

/// Where the performer was in the stream at one block: stream frames played before it, and his
/// take clock (frames rolled since Play) at its first frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeardMark {
    pub played: u64,
    pub rolled: i64,
    pub rolling: bool,
    /// The block played stream all through (it hadn't run dry): the mark means what it says.
    pub full: bool,
}

impl StreamIn {
    pub fn set_on(&self, on: bool) {
        self.on.store(on, Ordering::Release);
    }

    pub fn is_on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// The stream's volume in the performer's ears (linear, 0..4).
    pub fn set_gain(&self, g: f32) {
        let g = if g.is_finite() { g.clamp(0.0, 4.0) } else { 1.0 };
        self.gain.store((g * 1000.0) as u32, Ordering::Relaxed);
    }

    pub fn gain(&self) -> f32 {
        self.gain.load(Ordering::Relaxed) as f32 / 1000.0
    }

    /// The network side: queue interleaved stereo frames; returns how many frames fit.
    pub fn push(&self, samples: &[f32]) -> usize {
        let frames = samples.len() / 2;
        let fit = (self.ring.room() / 2).min(frames);
        self.ring.push(fit * 2, |i| samples.get(i).copied().map_or(0.0, crate::feed::finite)) / 2
    }

    /// Frames queued and not played yet.
    pub fn queued(&self) -> usize {
        self.ring.len() / 2
    }

    /// Stream frames played so far, and frames of silence because it ran dry.
    pub fn played(&self) -> u64 {
        self.played.load(Ordering::Relaxed)
    }

    pub fn starved(&self) -> u64 {
        self.starved.load(Ordering::Relaxed)
    }

    /// Drop what is queued (the audio thread does it at its next block).
    pub fn flush(&self) {
        self.flush.store(true, Ordering::Release);
    }

    /// The last block's mark, read consistently.
    pub fn mark(&self) -> HeardMark {
        for _ in 0..64 {
            let a = self.seq.load(Ordering::Acquire);
            if !a.is_multiple_of(2) {
                std::hint::spin_loop();
                continue;
            }
            let m = HeardMark {
                played: self.mark_played.load(Ordering::Relaxed),
                rolled: self.mark_rolled.load(Ordering::Relaxed),
                rolling: self.mark_rolling.load(Ordering::Relaxed),
                full: self.mark_full.load(Ordering::Relaxed),
            };
            // The data reads happen before the re-check (seqlock reader).
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == a {
                return m;
            }
        }
        HeardMark { played: self.mark_played.load(Ordering::Relaxed), rolled: self.mark_rolled.load(Ordering::Relaxed), rolling: false, full: false }
    }

    /// The audio thread: adds up to `frames` of the stream into `left` and `right` (mono: one
    /// slice, `right` empty) at take clock `rolled`. Never allocates or blocks.
    pub fn mix_into(&self, left: &mut [f32], right: &mut [f32], frames: usize, rolled: i64, rolling: bool) {
        if !self.on.load(Ordering::Relaxed) {
            return;
        }
        if self.flush.swap(false, Ordering::AcqRel) {
            // Dropped frames count as played: frame k pushed is always frame k played.
            let q = self.ring.len();
            let dropped = self.ring.skip(q - q % 2);
            self.played.fetch_add((dropped / 2) as u64, Ordering::Relaxed);
        }
        let played = self.played.load(Ordering::Relaxed);
        let full = self.ring.len() / 2 >= frames;
        let s = self.seq.load(Ordering::Relaxed);
        self.seq.store(s.wrapping_add(1), Ordering::Relaxed);
        // The odd count is seen before any of the data (seqlock writer).
        fence(Ordering::Release);
        self.mark_played.store(played, Ordering::Relaxed);
        self.mark_rolled.store(rolled, Ordering::Relaxed);
        self.mark_rolling.store(rolling, Ordering::Relaxed);
        self.mark_full.store(full, Ordering::Relaxed);
        self.seq.store(s.wrapping_add(2), Ordering::Release);
        let g = self.gain();
        let have = self.ring.len() / 2;
        let n = frames.min(have);
        let mono = right.is_empty();
        let taken = self.ring.pop_with(n * 2, |i, x| {
            let k = i / 2;
            if mono {
                if let Some(o) = left.get_mut(k) {
                    *o += 0.5 * x * g;
                }
            } else if i.is_multiple_of(2) {
                if let Some(o) = left.get_mut(k) {
                    *o += x * g;
                }
            } else if let Some(o) = right.get_mut(k) {
                *o += x * g;
            }
        });
        self.played.fetch_add((taken / 2) as u64, Ordering::Relaxed);
        if n < frames {
            self.starved.fetch_add((frames - n) as u64, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_blocks_come_out_whole_with_their_place() {
        let tap = MixTap::default();
        let l = [0.1f32, 0.2, 0.3];
        let r = [-0.1f32, -0.2, -0.3];
        tap.push_block(&l, &r, 3, 10, 5_000, true);
        assert_eq!(tap.ring.len(), 0, "off: nothing");
        tap.set_on(true);
        tap.push_block(&l, &r, 3, 10, 5_000, true);
        tap.push_block(&l, &l, 2, 13, -7, false);
        let mut out = Vec::new();
        tap.read(&mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], TapBlock { rolled: 10, position: 5_000, rolling: true, samples: vec![0.1, -0.1, 0.2, -0.2, 0.3, -0.3] });
        assert_eq!((out[1].rolled, out[1].position, out[1].rolling), (13, -7, false));
        assert_eq!(join(split(i64::MIN + 3).0, split(i64::MIN + 3).1), i64::MIN + 3);
    }

    #[test]
    fn the_stream_plays_in_order_and_marks_each_block() {
        let s = StreamIn::default();
        s.set_on(true);
        assert_eq!(s.push(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]), 3);
        s.set_gain(0.5);
        let (mut l, mut r) = (vec![0.0f32; 2], vec![0.0f32; 2]);
        s.mix_into(&mut l, &mut r, 2, 100, true);
        assert_eq!((l, r), (vec![0.5, 1.5], vec![1.0, 2.0]));
        let (mut l, mut r) = (vec![0.0f32; 2], vec![0.0f32; 2]);
        s.mix_into(&mut l, &mut r, 2, 102, true);
        assert_eq!(l, vec![2.5, 0.0], "then it runs dry");
        assert_eq!(s.mark(), HeardMark { played: 2, rolled: 102, rolling: true, full: false });
        assert_eq!((s.played(), s.starved()), (3, 1));
        s.push(&[1.0, 1.0]);
        s.flush();
        let (mut l, mut r) = (vec![0.0f32; 1], vec![0.0f32; 1]);
        s.mix_into(&mut l, &mut r, 1, 104, true);
        assert_eq!(l, vec![0.0], "flushed");
        assert_eq!(s.played(), 4, "a flushed frame counts as played");
    }
}
