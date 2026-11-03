//! Lock-free hand-off of live input to the output callback.
//!
//! The input callback pushes interleaved device frames into an [`InputFeed`]; the output callback
//! pops planar frames at the mix's rate. Both ends are allocation-free and lock-free: the ring's
//! slots are atomics (no `unsafe`), and the reader's own state lives in atomics only it touches.
//!
//! - [`InputFeed::fixed`]: input and output on one clock, called in a fixed order (ASIO: input,
//!   then output). Exactly the frames asked for are popped. Until the first input arrives the
//!   output is silence with no offset (the streams starting one after the other is no latency).
//!   The first pop with input starts at the newest frames (or, if that input falls short of the
//!   callback, plays the shortfall as silence: that offset is kept). Afterwards input that comes late is dropped once it catches up, and any surplus beyond the
//!   offset is skipped (no slack: the order is fixed), so the offset can't creep. No rate
//!   conversion.
//! - [`InputFeed::fixed_cushioned`]: the same on one device whose callbacks have no guaranteed
//!   order (WASAPI): one block is kept queued ahead, so an output callback that runs first, or
//!   twice in a row, still finds input, and a surplus is skipped only beyond one more block of
//!   slack (two inputs before an output is jitter there).
//! - [`InputFeed::drift`]: two clocks (two WASAPI devices). Linear interpolation, its ratio steered
//!   so the latency tracks twice the input callback size, or an output callback plus an input
//!   callback when that is more. The fill is refined with the time since the last push, so the
//!   steering sees drift long before a whole block slips. Its latency is that averaged fill (read
//!   it live: it moves with the callback sizes, and while bunched-up input drains).
//!
//! The output callback announces itself with [`InputFeed::begin`] (it pops exactly that many
//! frames: the direct path) or [`InputFeed::begin_elastic`] (the resampler, which pops whole mix
//! blocks ahead of what it plays, so a callback may pop none, one or several).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// Single-producer single-consumer FIFO of samples. The slots are atomics, so it needs no lock
/// and no `unsafe`.
pub(crate) struct SampleRing {
    slots: Box<[AtomicU32]>,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl SampleRing {
    pub(crate) fn new(capacity: usize) -> SampleRing {
        let n = capacity.clamp(2, 1 << 26).next_power_of_two();
        SampleRing { slots: (0..n).map(|_| AtomicU32::new(0)).collect(), mask: n - 1, head: AtomicUsize::new(0), tail: AtomicUsize::new(0) }
    }

    /// Samples queued.
    pub fn len(&self) -> usize {
        self.head.load(Ordering::Acquire).wrapping_sub(self.tail.load(Ordering::Acquire)).min(self.slots.len())
    }

    /// Room left, in samples (producer side).
    pub(crate) fn room(&self) -> usize {
        self.free()
    }

    /// Room left, in samples (producer side).
    fn free(&self) -> usize {
        self.slots.len().saturating_sub(self.head.load(Ordering::Relaxed).wrapping_sub(self.tail.load(Ordering::Acquire)))
    }

    /// Queue up to `n` samples (`sample(i)` is the i-th); returns how many fit.
    pub(crate) fn push(&self, n: usize, sample: impl Fn(usize) -> f32) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let n = n.min(self.free());
        for i in 0..n {
            if let Some(slot) = self.slots.get(head.wrapping_add(i) & self.mask) {
                slot.store(sample(i).to_bits(), Ordering::Relaxed);
            }
        }
        self.head.store(head.wrapping_add(n), Ordering::Release);
        n
    }

    /// The `i`-th queued sample, without taking it (consumer side; 0 past the end).
    pub(crate) fn peek(&self, i: usize) -> f32 {
        if i >= self.len() {
            return 0.0;
        }
        let tail = self.tail.load(Ordering::Relaxed);
        self.slots.get(tail.wrapping_add(i) & self.mask).map_or(0.0, |s| f32::from_bits(s.load(Ordering::Relaxed)))
    }

    /// Take up to `n` samples, handing each to `each(i, sample)`; returns how many.
    pub(crate) fn pop_with(&self, n: usize, mut each: impl FnMut(usize, f32)) -> usize {
        let (tail, head) = (self.tail.load(Ordering::Relaxed), self.head.load(Ordering::Acquire));
        let n = n.min(head.wrapping_sub(tail)).min(self.slots.len());
        for i in 0..n {
            each(i, self.slots.get(tail.wrapping_add(i) & self.mask).map_or(0.0, |s| f32::from_bits(s.load(Ordering::Relaxed))));
        }
        self.tail.store(tail.wrapping_add(n), Ordering::Release);
        n
    }

    /// Drop up to `n` samples; returns how many.
    pub(crate) fn skip(&self, n: usize) -> usize {
        self.pop_with(n, |_, _| {})
    }

    /// Take exactly what fits in `out` (or all there is); returns how many samples.
    pub fn pop_exact(&self, out: &mut [f32]) -> usize {
        let n = out.len();
        self.pop_with(n, |i, x| {
            if let Some(o) = out.get_mut(i) {
                *o = x;
            }
        })
    }

    /// Take everything queued (off the audio thread: it may grow `out`).
    pub(crate) fn pop_all(&self, out: &mut Vec<f32>) {
        let n = self.len();
        out.reserve(n);
        self.pop_with(n, |_, x| out.push(x));
    }

    /// Forget everything queued (consumer side).
    pub(crate) fn clear(&self) {
        self.tail.store(self.head.load(Ordering::Acquire), Ordering::Release);
    }
}

/// Remembers the largest recent callback size: a bigger one is taken at once, a smaller one once
/// the bigger hasn't come for [`BLOCK_MEMORY`] callbacks.
fn remember_largest(largest: &AtomicUsize, age: &AtomicUsize, frames: usize) {
    let a = age.load(Ordering::Relaxed);
    if frames >= largest.load(Ordering::Relaxed) || a >= BLOCK_MEMORY {
        largest.store(frames, Ordering::Relaxed);
        age.store(0, Ordering::Relaxed);
    } else {
        age.store(a + 1, Ordering::Relaxed);
    }
}

/// An input sample, or silence for NaN and infinities, which would poison the take and the mix.
pub(crate) fn finite(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// An `f64` the reader keeps between calls.
struct F64(AtomicU64);

impl F64 {
    fn new(v: f64) -> F64 {
        F64(AtomicU64::new(v.to_bits()))
    }
    fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }
    fn set(&self, v: f64) {
        self.0.store(v.to_bits(), Ordering::Relaxed);
    }
}

/// Proportional gain of the drift steering: a ratio change per input frame of fill error.
const KP: f64 = 5e-6;
/// Integral gain: a ratio change per input frame·second of error.
const KI: f64 = 5e-7;
/// The steering never moves the ratio further than this from nominal (0.2 %).
const MAX_ADJUST: f64 = 2e-3;
/// Time constant of the averaged fill, in seconds: it smooths what jitter is left.
const AVERAGE_SECONDS: f64 = 0.5;
/// Callbacks after which the drift target forgets a larger input (or output) callback.
const BLOCK_MEMORY: usize = 64;
/// Fill beyond the target (in input callbacks) after which the reader skips back to it.
const TOO_FULL_BLOCKS: usize = 4;

/// The reader's state of a drift feed (only the output callback touches it).
struct Drift {
    /// Input frames per output frame at nominal rates.
    nominal: f64,
    in_rate: f64,
    out_rate: f64,
    /// Steering: the averaged fill (input frames), the error integral, the read phase between
    /// `cur` and the ring's first frame.
    avg: F64,
    integral: F64,
    phase: F64,
    adjust: F64,
    /// The input frame being read from (per channel).
    cur: Box<[AtomicU32]>,
}

enum Kind {
    Fixed {
        /// Frames output as silence for input that hadn't come (after the first pop): input that
        /// comes late is dropped up to this, so the offset stays. Forgotten as soon as a pop is
        /// served in full without a late surplus (that input was lost, not late).
        owed: AtomicUsize,
        /// Keep one block queued ahead (callbacks with no guaranteed order).
        cushion: bool,
        /// Frames of the current output callback not popped yet ([`InputFeed::begin`]).
        budget: AtomicUsize,
        /// Silence to play before the input this callback (the offset found on the first pop).
        lead: AtomicUsize,
    },
    Drift(Drift),
}

/// Live input from an input callback to the output callback (see the module docs).
pub struct InputFeed {
    ring: SampleRing,
    channels: usize,
    kind: Kind,
    /// The extra latency this feed adds, in output frames: the fixed offset, or the averaged
    /// drift fill.
    extra: AtomicU32,
    /// The reader has started (fixed: the offset is set; drift: the target fill was reached).
    primed: AtomicBool,
    /// Input has arrived at least once.
    pushed: AtomicBool,
    /// The largest recent input callback, in frames, and the pushes since it was last seen.
    block: AtomicUsize,
    block_age: AtomicUsize,
    /// The largest recent output callback (in popped frames), and the callbacks since it was
    /// last seen.
    out_block: AtomicUsize,
    out_age: AtomicUsize,
    /// When the last input arrived (ns on the feed's clock; 0: never, or no clock).
    pushed_at: AtomicU64,
    /// Pops that ran out of input after the reader started (drift).
    underflows: AtomicU64,
    /// Pushes that didn't fit in full.
    overflows: AtomicU64,
    /// The input callback's reported latency (capture to callback), in ns.
    input_latency_ns: AtomicU64,
    #[cfg(not(target_arch = "wasm32"))]
    clock: std::time::Instant,
}

impl InputFeed {
    fn new(channels: usize, capacity_frames: usize, kind: Kind) -> InputFeed {
        let channels = channels.clamp(1, 256);
        InputFeed {
            ring: SampleRing::new(capacity_frames.saturating_mul(channels)),
            channels,
            kind,
            extra: AtomicU32::new(0),
            primed: AtomicBool::new(false),
            pushed: AtomicBool::new(false),
            block: AtomicUsize::new(0),
            block_age: AtomicUsize::new(0),
            out_block: AtomicUsize::new(0),
            out_age: AtomicUsize::new(0),
            pushed_at: AtomicU64::new(0),
            underflows: AtomicU64::new(0),
            overflows: AtomicU64::new(0),
            input_latency_ns: AtomicU64::new(0),
            #[cfg(not(target_arch = "wasm32"))]
            clock: std::time::Instant::now(),
        }
    }

    /// Now on the feed's clock, in ns (never 0); 0 where there is no clock (the web).
    fn now_ns(&self) -> u64 {
        #[cfg(not(target_arch = "wasm32"))]
        {
            u64::try_from(self.clock.elapsed().as_nanos()).unwrap_or(u64::MAX).saturating_add(1)
        }
        #[cfg(target_arch = "wasm32")]
        {
            0
        }
    }

    /// One clock: the reader pops exactly the frames asked for (see the module docs). Holds
    /// 8 × `max_frames`.
    pub fn fixed(channels: usize, max_frames: usize) -> Arc<InputFeed> {
        InputFeed::fixed_with(channels, max_frames, false)
    }

    /// One clock, callbacks in no guaranteed order: as [`InputFeed::fixed`], with one block kept
    /// queued ahead (`extra_latency` is that block).
    pub fn fixed_cushioned(channels: usize, max_frames: usize) -> Arc<InputFeed> {
        InputFeed::fixed_with(channels, max_frames, true)
    }

    fn fixed_with(channels: usize, max_frames: usize, cushion: bool) -> Arc<InputFeed> {
        let kind = Kind::Fixed { owed: AtomicUsize::new(0), cushion, budget: AtomicUsize::new(0), lead: AtomicUsize::new(0) };
        Arc::new(InputFeed::new(channels, max_frames.clamp(1, 1 << 16).saturating_mul(8), kind))
    }

    /// Two clocks: input at `in_rate` read at `out_rate`, the ratio steered to keep the fill at
    /// twice the input callback size, or an output callback (in input frames) plus an input
    /// callback when that is more. Holds 8 × the larger of `max_frames` and 4096 (the input
    /// device's buffer is its own).
    pub fn drift(channels: usize, in_rate: u32, out_rate: u32, max_frames: usize) -> Arc<InputFeed> {
        let (in_rate, out_rate) = (f64::from(in_rate.max(1)), f64::from(out_rate.max(1)));
        let ch = channels.clamp(1, 256);
        let drift = Drift {
            nominal: in_rate / out_rate,
            in_rate,
            out_rate,
            avg: F64::new(0.0),
            integral: F64::new(0.0),
            phase: F64::new(0.0),
            adjust: F64::new(0.0),
            cur: (0..ch).map(|_| AtomicU32::new(0)).collect(),
        };
        Arc::new(InputFeed::new(ch, max_frames.clamp(4096, 1 << 16).saturating_mul(8), Kind::Drift(drift)))
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Input callbacks that didn't fit in full (the output stopped reading).
    pub fn overflows(&self) -> u64 {
        self.overflows.load(Ordering::Relaxed)
    }

    /// Which feed this is, for tests.
    #[cfg(test)]
    pub(crate) fn describe(&self) -> &'static str {
        match &self.kind {
            Kind::Fixed { cushion: false, .. } => "fixed",
            Kind::Fixed { cushion: true, .. } => "cushioned",
            Kind::Drift(_) => "drift",
        }
    }

    #[cfg(test)]
    fn block_size(&self) -> usize {
        self.block.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn read_position(&self) -> f64 {
        let consumed = self.ring.tail.load(Ordering::Relaxed) / self.channels;
        let phase = match &self.kind {
            Kind::Drift(d) => d.phase.get(),
            Kind::Fixed { .. } => 0.0,
        };
        consumed as f64 - 1.0 + phase
    }

    #[cfg(test)]
    fn adjust(&self) -> f64 {
        match &self.kind {
            Kind::Drift(d) => d.adjust.get(),
            Kind::Fixed { .. } => 0.0,
        }
    }

    /// The latency this feed adds, in output frames.
    pub fn extra_latency(&self) -> u32 {
        self.extra.load(Ordering::Relaxed)
    }

    /// The input callback stores its reported latency (capture to callback) here.
    pub fn set_input_latency(&self, d: std::time::Duration) {
        self.input_latency_ns.store(u64::try_from(d.as_nanos()).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    /// The input callback's reported latency, as last stored.
    pub fn input_latency(&self) -> std::time::Duration {
        std::time::Duration::from_nanos(self.input_latency_ns())
    }

    pub(crate) fn input_latency_ns(&self) -> u64 {
        self.input_latency_ns.load(Ordering::Relaxed)
    }

    /// Times a started drift feed ran dry.
    pub fn underflows(&self) -> u64 {
        self.underflows.load(Ordering::Relaxed)
    }

    /// Input frames queued.
    pub fn queued(&self) -> usize {
        self.ring.len() / self.channels
    }

    /// Queue an input callback's interleaved frames (whole frames only; what doesn't fit is
    /// dropped). NaN and infinities become silence. The input callback's end.
    pub fn push(&self, interleaved: &[f32]) {
        self.push_at(interleaved, self.now_ns());
    }

    /// [`InputFeed::push`] at `now_ns` on the feed's clock.
    pub(crate) fn push_at(&self, interleaved: &[f32], now_ns: u64) {
        let ch = self.channels;
        let frames = interleaved.len() / ch;
        if frames == 0 {
            return;
        }
        remember_largest(&self.block, &self.block_age, frames);
        let fit = (self.ring.free() / ch).min(frames) * ch;
        if fit < frames * ch {
            self.overflows.fetch_add(1, Ordering::Relaxed);
        }
        self.ring.push(fit, |i| interleaved.get(i).copied().map_or(0.0, finite));
        self.pushed_at.store(now_ns, Ordering::Release);
        self.pushed.store(true, Ordering::Release);
    }

    /// Fill `frames` of the planar `out` with input; missing frames (and channels the feed lacks)
    /// are silence. The output callback's end.
    pub fn pop_into(&self, out: &mut [Vec<f32>], frames: usize) {
        self.pop_at(out, frames, self.now_ns());
    }

    /// [`InputFeed::pop_into`] at `now_ns` on the feed's clock.
    pub(crate) fn pop_at(&self, out: &mut [Vec<f32>], frames: usize, now_ns: u64) {
        for c in out.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
        match &self.kind {
            Kind::Fixed { owed, budget, lead, .. } => {
                // A pop outside a callback announced with `begin` is a callback of its own.
                if budget.load(Ordering::Relaxed) == 0 {
                    self.begin(frames);
                }
                budget.store(budget.load(Ordering::Relaxed).saturating_sub(frames), Ordering::Relaxed);
                self.pop_fixed(owed, lead, out, frames);
            }
            Kind::Drift(d) => self.pop_drift(d, out, frames, now_ns),
        }
    }

    /// An output callback that will pop `frames` in all (perhaps in several pops) starts: a
    /// fixed feed lines the queue up for it once, so chunked pops aren't each taken for a whole
    /// callback. A drift feed keeps its fill above the output callback.
    pub fn begin(&self, frames: usize) {
        match &self.kind {
            Kind::Fixed { owed, cushion, budget, lead } => {
                budget.store(frames, Ordering::Relaxed);
                self.align(owed, *cushion, lead, frames);
            }
            Kind::Drift(_) => remember_largest(&self.out_block, &self.out_age, frames),
        }
    }

    /// A resampling output callback starts: it will pop about `pops` frames, in whole mix blocks
    /// rendered ahead of what it plays (so none in one callback and two in the next), while
    /// `arrive` frames of input come per callback. Pops don't match arrivals callback by
    /// callback (and under varispeed not even on average), so a fixed feed doesn't trim the
    /// queue to the callback: input stays queued for the block that needs it, and a surplus is
    /// skipped only beyond what this callback pops plus one input callback (and the offset).
    /// The first callback lines up as [`InputFeed::begin`] does, for `arrive`.
    pub fn begin_elastic(&self, pops: usize, arrive: usize) {
        match &self.kind {
            Kind::Fixed { owed, cushion, budget, lead } => {
                if !self.primed.load(Ordering::Relaxed) {
                    self.align(owed, *cushion, lead, arrive);
                } else {
                    let ch = self.channels;
                    let avail = self.ring.len() / ch;
                    let ceiling = pops.max(arrive).saturating_add(arrive).saturating_add(self.extra.load(Ordering::Relaxed) as usize);
                    if avail > ceiling {
                        self.ring.skip((avail - ceiling) * ch);
                    }
                    // Input missing now is not late input to drop later: the pops ran ahead.
                    owed.store(0, Ordering::Relaxed);
                }
                // Every pop until the next callback is part of this one, however many.
                budget.store(usize::MAX, Ordering::Relaxed);
            }
            Kind::Drift(_) => remember_largest(&self.out_block, &self.out_age, pops),
        }
    }

    /// Copy `n` frames from the ring into `out[at..]`.
    fn take_frames(&self, out: &mut [Vec<f32>], at: usize, n: usize) {
        let ch = self.channels;
        self.ring.pop_with(n * ch, |i, x| {
            if let Some(o) = out.get_mut(i % ch).and_then(|c| c.get_mut(at + i / ch)) {
                *o = x;
            }
        });
    }

    /// Line the queue up for an output callback of `frames`: the first time, start at the newest
    /// frames (or keep the output's lead as the offset, or queue a block ahead); afterwards drop
    /// a surplus beyond the offset (and its slack), and late input already played as silence.
    fn align(&self, owed: &AtomicUsize, cushion: bool, lead: &AtomicUsize, frames: usize) {
        let ch = self.channels;
        let avail = self.ring.len() / ch;
        if !self.primed.load(Ordering::Relaxed) {
            if cushion {
                // Wait for one block, then keep the newest block queued ahead: silence now.
                lead.store(frames, Ordering::Relaxed);
                if avail >= frames && frames > 0 {
                    self.ring.skip((avail - frames) * ch);
                    self.extra.store(u32::try_from(frames).unwrap_or(u32::MAX), Ordering::Relaxed);
                    self.primed.store(true, Ordering::Relaxed);
                }
                return;
            }
            // No input yet: silence, and no offset. The order is fixed (ASIO: input, then output)
            // once both streams run; an output callback before the input stream's first is the
            // two streams starting (a race), and taking it as the offset would add a block of
            // latency for good, on some opens and not others.
            if !self.pushed.load(Ordering::Acquire) {
                lead.store(0, Ordering::Relaxed);
                return;
            }
            // Start at the newest frames; if the first input falls short of the output callback,
            // the shortfall is the offset from now on: silence first, then the input.
            self.ring.skip(avail.saturating_sub(frames) * ch);
            let missing = frames.saturating_sub(avail);
            lead.store(missing, Ordering::Relaxed);
            self.extra.store(u32::try_from(missing).unwrap_or(u32::MAX), Ordering::Relaxed);
            self.primed.store(true, Ordering::Relaxed);
            return;
        }
        let expected = frames + self.extra.load(Ordering::Relaxed) as usize;
        let mut debt = owed.load(Ordering::Relaxed);
        // One block of slack where callbacks may come in any order (two inputs before an
        // output is jitter there); none where the order is fixed.
        let slack = if cushion { frames } else { 0 };
        if avail > expected + slack {
            // A surplus queued (the output read slower for a while, or the input bunched up):
            // back to the offset.
            self.ring.skip((avail - expected) * ch);
            debt = 0;
        } else if avail > expected && debt > 0 {
            // Input that came late, for frames already played as silence: dropped, so the offset
            // stays.
            let skip = (avail - expected).min(debt);
            self.ring.skip(skip * ch);
            debt -= skip;
        } else if avail >= frames {
            // Served in full with nothing late: what was owed was lost, not late.
            debt = 0;
        }
        owed.store(debt, Ordering::Relaxed);
    }

    fn pop_fixed(&self, owed: &AtomicUsize, lead: &AtomicUsize, out: &mut [Vec<f32>], frames: usize) {
        let ch = self.channels;
        let silent = lead.load(Ordering::Relaxed).min(frames);
        lead.store(lead.load(Ordering::Relaxed) - silent, Ordering::Relaxed);
        let take = (self.ring.len() / ch).min(frames - silent);
        let missing = frames - silent - take;
        self.take_frames(out, silent, take);
        // Input missing (late, or the device stopped): what there is, then silence. Only owed
        // once input has started at all, and never more than the ring holds.
        if missing > 0 && self.primed.load(Ordering::Relaxed) && self.pushed.load(Ordering::Acquire) {
            let debt = owed.load(Ordering::Relaxed).saturating_add(missing).min(self.ring.slots.len() / ch);
            owed.store(debt, Ordering::Relaxed);
        }
    }

    fn pop_drift(&self, d: &Drift, out: &mut [Vec<f32>], frames: usize, now_ns: u64) {
        let ch = self.channels;
        let block = self.block.load(Ordering::Relaxed);
        if block == 0 || frames == 0 {
            return;
        }
        let cap = self.ring.slots.len() / ch;
        // At least an output callback (in input frames) plus an input callback: at the pop the
        // queue is the target less the input on its way (up to an input callback).
        let out_frames = (self.out_block.load(Ordering::Relaxed).max(frames) as f64 * d.nominal).ceil() as usize;
        let target = (2 * block).max(out_frames.saturating_add(block)).min(cap / 2).max(1);
        let mut fill = self.ring.len() / ch;
        if !self.primed.load(Ordering::Relaxed) {
            if fill < target {
                return;
            }
            // Start reading: from the first queued frame, at the target fill.
            self.ring.skip((fill - target) * ch);
            self.ring.pop_with(ch, |i, x| {
                if let Some(c) = d.cur.get(i) {
                    c.store(x.to_bits(), Ordering::Relaxed);
                }
            });
            fill = target.saturating_sub(1);
            d.phase.set(0.0);
            d.avg.set(fill as f64);
            d.integral.set(0.0);
            d.adjust.set(0.0);
            self.primed.store(true, Ordering::Relaxed);
        }
        // The output stopped reading for a while: back to the target rather than late for good.
        if fill > target + TOO_FULL_BLOCKS * block {
            self.ring.skip((fill - target) * ch);
            fill = target;
            d.avg.set(fill as f64);
        }
        // Steer: the averaged fill towards the target. The fill counts the input captured since
        // the last push too (it is on its way): that grows smoothly between pushes and falls by
        // as much as a push adds, so the measure has no block-sized steps and drift shows within
        // seconds, not when a whole block slips. Without a clock it is the queued fill alone.
        let since = match self.pushed_at.load(Ordering::Acquire) {
            0 => 0.0,
            at if now_ns > at => (now_ns - at) as f64 * 1e-9,
            _ => 0.0,
        };
        let arriving = (since * d.in_rate).min(2.0 * block as f64);
        let measured = fill as f64 + arriving;
        let dt = frames as f64 / d.out_rate;
        let alpha = (dt / AVERAGE_SECONDS).min(1.0);
        let avg = d.avg.get() + (measured - d.avg.get()) * alpha;
        d.avg.set(avg);
        // The latency is the averaged fill, in output frames: what the input really waits, which
        // the steering holds at the target but which moves with it (the callback sizes) and
        // away from it for a while (input that bunched up drains slowly).
        let extra = (avg / d.nominal).round();
        self.extra.store(if extra > 0.0 { extra.min(f64::from(u32::MAX)) as u32 } else { 0 }, Ordering::Relaxed);
        let err = avg - target as f64;
        let integral = (d.integral.get() + err * dt).clamp(-MAX_ADJUST / KI, MAX_ADJUST / KI);
        d.integral.set(integral);
        let adjust = (KP * err + KI * integral).clamp(-MAX_ADJUST, MAX_ADJUST);
        d.adjust.set(adjust);
        let step = d.nominal * (1.0 + adjust);
        let mut phase = d.phase.get();
        for f in 0..frames {
            if self.ring.len() < ch {
                // Ran dry: silence, and wait for the target fill again.
                self.underflows.fetch_add(1, Ordering::Relaxed);
                self.primed.store(false, Ordering::Relaxed);
                break;
            }
            let fr = phase as f32;
            for (c, cur) in d.cur.iter().enumerate() {
                let a = f32::from_bits(cur.load(Ordering::Relaxed));
                let b = self.ring.peek(c);
                if let Some(o) = out.get_mut(c).and_then(|v| v.get_mut(f)) {
                    *o = a + (b - a) * fr;
                }
            }
            phase += step;
            while phase >= 1.0 && self.ring.len() >= ch {
                self.ring.pop_with(ch, |i, x| {
                    if let Some(c) = d.cur.get(i) {
                        c.store(x.to_bits(), Ordering::Relaxed);
                    }
                });
                phase -= 1.0;
            }
        }
        d.phase.set(phase.min(1.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planar(ch: usize, n: usize) -> Vec<Vec<f32>> {
        vec![vec![0.0; n]; ch]
    }

    fn interleave(frames: &[[f32; 2]]) -> Vec<f32> {
        frames.iter().flatten().copied().collect()
    }

    #[test]
    fn fixed_feed_hands_over_one_buffer_unchanged() {
        let feed = InputFeed::fixed(2, 128);
        let frames: Vec<[f32; 2]> = (0..128).map(|i| [i as f32, -(i as f32)]).collect();
        feed.push(&interleave(&frames));
        let mut out = planar(2, 128);
        feed.pop_into(&mut out, 128);
        assert_eq!(out[0], frames.iter().map(|f| f[0]).collect::<Vec<_>>());
        assert_eq!(out[1], frames.iter().map(|f| f[1]).collect::<Vec<_>>());
        assert_eq!((feed.extra_latency(), feed.queued()), (0, 0));
    }

    #[test]
    fn a_start_race_adds_no_latency() {
        // ASIO: the output callback can run before the input stream's first callback (the
        // streams start one after the other). That silence is no offset: the feed waits for its
        // first input, then the order is fixed (input, then output) and nothing is late.
        let feed = InputFeed::fixed(2, 128);
        let mut out = planar(2, 128);
        let (mut got, mut sent) = (Vec::new(), Vec::new());
        for _ in 0..3 {
            feed.pop_into(&mut out, 128);
            assert!(out.iter().flatten().all(|x| *x == 0.0), "no input yet: silence");
        }
        for b in 0..1000 {
            let frames: Vec<[f32; 2]> = (0..128).map(|i| [(b * 128 + i + 1) as f32, 0.5]).collect();
            sent.extend(frames.iter().map(|f| f[0]));
            feed.push(&interleave(&frames));
            feed.pop_into(&mut out, 128);
            got.extend_from_slice(&out[0]);
            assert!(out[1].iter().all(|x| *x == 0.5));
        }
        assert_eq!(got, sent, "every frame, in the callback it arrived for");
        assert_eq!(feed.extra_latency(), 0);
    }

    #[test]
    fn fixed_feed_keeps_a_constant_offset_when_the_first_input_falls_short() {
        // The input has started, but its first callback is shorter than the output's: the
        // shortfall is the offset from then on.
        let feed = InputFeed::fixed(1, 128);
        let mut out = planar(1, 128);
        feed.push(&ramp(0, 32));
        feed.pop_into(&mut out, 128);
        assert_eq!(&out[0][..96], &[0.0; 96][..]);
        assert_eq!(&out[0][96..], &ramp(0, 32)[..]);
        assert_eq!(feed.extra_latency(), 96);
        for b in 0..100 {
            feed.push(&ramp(32 + b * 128, 128));
            feed.pop_into(&mut out, 128);
            assert_eq!(out[0], ramp(32 + b * 128, 128), "96 frames late, every frame");
        }
    }

    /// Input at `in_rate` × (1 + `ppm`), read at `out_rate`, both in 480-frame callbacks, for
    /// `seconds`. Returns the output and the fill seen before each pop (after 2 s).
    fn drift_run(ppm: f64, seconds: f64) -> (Arc<InputFeed>, Vec<f32>, Vec<usize>) {
        let (in_rate, out_rate, block) = (48_000.0f64, 48_000.0f64, 480usize);
        let true_in = in_rate * (1.0 + ppm * 1e-6);
        let feed = InputFeed::drift(1, in_rate as u32, out_rate as u32, 512);
        let (mut out, mut fills) = (Vec::new(), Vec::new());
        let mut buf = planar(1, block);
        let (mut ins, mut outs) = (0usize, 0usize);
        let tone = |k: usize| (2.0 * std::f64::consts::PI * 1000.0 * k as f64 / true_in).sin() as f32;
        let mut input: Vec<f32> = Vec::with_capacity(block);
        while (outs * block) as f64 / out_rate < seconds {
            let t_in = (ins * block) as f64 / true_in;
            let t_out = (outs * block) as f64 / out_rate;
            if t_in <= t_out {
                input.clear();
                input.extend((0..block).map(|i| tone(ins * block + i)));
                feed.push(&input);
                ins += 1;
            } else {
                if t_out >= 2.0 {
                    fills.push(feed.queued());
                }
                feed.pop_into(&mut buf, block);
                out.extend_from_slice(&buf[0]);
                outs += 1;
            }
        }
        (feed, out, fills)
    }

    /// Least squares: the coefficients of `basis` that best fit `x`, by the normal equations.
    #[allow(clippy::needless_range_loop)] // matrix maths reads clearer with indices
    fn fit<const N: usize>(x: &[f32], basis: impl Fn(usize) -> [f64; N]) -> [f64; N] {
        let (mut m, mut v) = ([[0.0f64; N]; N], [0.0f64; N]);
        for (k, y) in x.iter().enumerate() {
            let b = basis(k);
            for i in 0..N {
                v[i] += b[i] * f64::from(*y);
                for j in 0..N {
                    m[i][j] += b[i] * b[j];
                }
            }
        }
        // Gauss-Jordan (the matrix is symmetric positive definite).
        for c in 0..N {
            let p = m[c][c];
            for j in 0..N {
                m[c][j] /= p;
            }
            v[c] /= p;
            for r in 0..N {
                if r != c {
                    let f = m[r][c];
                    for j in 0..N {
                        m[r][j] -= f * m[c][j];
                    }
                    v[r] -= f * v[c];
                }
            }
        }
        v
    }

    /// THD+N of a ~1 kHz sine at 48 kHz, in dB: the residual once the fundamental is removed, over
    /// windows of 4800. The fundamental is fitted with its amplitude and a slowly moving phase
    /// (sine and cosine, and both times a ramp), so a few hundred ppm off 1 kHz is not counted:
    /// a pitch offset is not distortion.
    fn thd_n_db(x: &[f32]) -> f64 {
        let w0 = 2.0 * std::f64::consts::PI * 1000.0 / 48_000.0;
        let (mut sig, mut res) = (0.0f64, 0.0f64);
        for (w, win) in x.chunks_exact(4800).enumerate() {
            let basis = |i: usize| {
                let k = (w * 4800 + i) as f64;
                let ramp = (i as f64 - 2400.0) / 2400.0;
                let (s, c) = ((w0 * k).sin(), (w0 * k).cos());
                [s, c, ramp * s, ramp * c]
            };
            let a = fit(win, basis);
            for (i, v) in win.iter().enumerate() {
                let b = basis(i);
                let f: f64 = a.iter().zip(b).map(|(a, b)| a * b).sum();
                sig += f * f;
                res += (f64::from(*v) - f).powi(2);
            }
        }
        10.0 * (res / sig).log10()
    }

    #[test]
    fn drift_feed_tracks_a_fast_and_a_slow_input() {
        // ±1000 ppm drifts 2880 frames in 60 s: only the steering keeps it within one callback.
        for ppm in [100.0, -100.0, 1000.0, -1000.0] {
            let (feed, out, fills) = drift_run(ppm, 60.0);
            let target = 2 * 480;
            let worst = fills.iter().map(|f| f.abs_diff(target)).max().unwrap();
            assert!(worst <= 480, "{ppm} ppm: the fill strays {worst} frames from {target}");
            assert_eq!(feed.underflows(), 0, "{ppm} ppm: ran dry");
            let extra = feed.extra_latency();
            assert!(extra.abs_diff(960) <= 480, "{ppm} ppm: the latency is the fill held at the target: {extra}");
            // From 2 s on (the start is silence until the target fill).
            let thd = thd_n_db(&out[96_000..]);
            // (At ±1000 ppm the correction itself bends the pitch a little when a block slips.)
            assert!(thd < if ppm.abs() > 500.0 { -35.0 } else { -40.0 }, "{ppm} ppm: THD+N {thd:.1} dB");
        }
    }

    #[test]
    fn feeds_do_not_allocate() {
        for feed in [InputFeed::fixed(2, 256), InputFeed::drift(2, 44_100, 48_000, 256)] {
            let mut out = planar(2, 256);
            let input = vec![0.25f32; 2 * 256];
            let before: Vec<(usize, *const f32)> = out.iter().map(|c| (c.capacity(), c.as_ptr())).collect();
            for i in 0..10_000 {
                feed.push(&input[..2 * (64 + i % 192)]);
                feed.pop_into(&mut out, 64 + (i * 7) % 192);
            }
            let after: Vec<(usize, *const f32)> = out.iter().map(|c| (c.capacity(), c.as_ptr())).collect();
            assert_eq!(before, after);
            assert_eq!(out.capacity(), 2);
        }
    }

    #[test]
    fn non_finite_input_is_silence() {
        let feed = InputFeed::fixed(1, 8);
        feed.push(&[0.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -0.5]);
        let mut out = planar(1, 5);
        feed.pop_into(&mut out, 5);
        assert_eq!(out[0], [0.5, 0.0, 0.0, 0.0, -0.5]);
    }

    #[test]
    fn a_late_block_is_dropped_so_the_offset_stays() {
        let feed = InputFeed::fixed(1, 16);
        let mut out = planar(1, 4);
        feed.push(&[1.0; 4]);
        feed.pop_into(&mut out, 4);
        feed.pop_into(&mut out, 4);
        assert_eq!(out[0], [0.0; 4], "the input is late: silence");
        feed.push(&[2.0; 4]);
        feed.push(&[3.0; 4]);
        feed.pop_into(&mut out, 4);
        assert_eq!(out[0], [3.0; 4], "the late block was played as silence: dropped");
        feed.push(&[4.0; 4]);
        feed.pop_into(&mut out, 4);
        assert_eq!((out[0].as_slice(), feed.queued()), (&[4.0; 4][..], 0));
    }

    #[test]
    fn sample_ring_pops_exactly_what_fits() {
        let ring = SampleRing::new(8);
        assert_eq!(ring.push(10, |i| i as f32), 8);
        let mut out = [0.0; 3];
        assert_eq!(ring.pop_exact(&mut out), 3);
        assert_eq!((out, ring.len()), ([0.0, 1.0, 2.0], 5));
        let mut rest = Vec::new();
        ring.pop_all(&mut rest);
        assert_eq!(rest, [3.0, 4.0, 5.0, 6.0, 7.0]);
    }

    /// `n` frames of a mono ramp from `from`.
    fn ramp(from: usize, n: usize) -> Vec<f32> {
        (from..from + n).map(|i| (i + 1) as f32).collect()
    }

    #[test]
    fn the_first_pop_takes_the_newest_block() {
        let feed = InputFeed::fixed(1, 128);
        for b in 0..3 {
            feed.push(&ramp(b * 128, 128));
        }
        let mut out = planar(1, 128);
        feed.pop_into(&mut out, 128);
        assert_eq!(out[0], ramp(256, 128), "the newest block, not the oldest");
        assert_eq!((feed.extra_latency(), feed.queued()), (0, 0));
        feed.push(&ramp(384, 128));
        feed.pop_into(&mut out, 128);
        assert_eq!(out[0], ramp(384, 128), "and no latency from then on");
    }

    #[test]
    fn a_fixed_feed_recovers_after_a_slow_stretch() {
        // Varispeed 0.5: the mix pops half of what arrives. Afterwards the latency is back to none.
        let feed = InputFeed::fixed(1, 128);
        let mut out = planar(1, 128);
        let mut at = 0;
        for _ in 0..50 {
            feed.push(&ramp(at, 128));
            at += 128;
            feed.pop_into(&mut out, 64);
        }
        for _ in 0..3 {
            feed.push(&ramp(at, 128));
            feed.pop_into(&mut out, 128);
            assert_eq!(out[0], ramp(at, 128), "the block just pushed");
            at += 128;
        }
        assert_eq!(feed.queued(), 0);
    }

    #[test]
    fn a_missing_blocks_debt_does_not_drop_a_later_block() {
        // The first input came one frame into a 129-frame callback (offset 128: two blocks may
        // be queued legitimately).
        let feed = InputFeed::fixed(1, 129);
        let mut out = planar(1, 129);
        feed.push(&[9.0]);
        feed.pop_into(&mut out, 129);
        assert_eq!(feed.extra_latency(), 128);
        let mut out = planar(1, 128);
        let mut at = 0;
        for _ in 0..3 {
            feed.push(&ramp(at, 128));
            feed.pop_into(&mut out, 128);
            assert_eq!(out[0], ramp(at, 128));
            at += 128;
        }
        // A block lost for good: silence, owed for a moment.
        feed.pop_into(&mut out, 128);
        assert_eq!(out[0], [0.0; 128]);
        at += 128;
        for _ in 0..3 {
            feed.push(&ramp(at, 128));
            feed.pop_into(&mut out, 128);
            assert_eq!(out[0], ramp(at, 128));
            at += 128;
        }
        // Later two input callbacks before an output callback, within the offset: both play.
        feed.push(&ramp(at, 128));
        feed.push(&ramp(at + 128, 128));
        feed.pop_into(&mut out, 128);
        assert_eq!(out[0], ramp(at, 128), "the older of the two plays, not skipped as owed");
        feed.pop_into(&mut out, 128);
        assert_eq!(out[0], ramp(at + 128, 128));
    }

    #[test]
    fn a_cushioned_feed_rides_out_callback_order_jitter() {
        // Same device, no guaranteed order (WASAPI): one block queued ahead absorbs output
        // callbacks that run before, or twice between, input callbacks.
        let feed = InputFeed::fixed_cushioned(1, 128);
        let mut out = planar(1, 128);
        let (mut pushed, mut got) = (0, Vec::new());
        let mut push = |feed: &InputFeed| {
            feed.push(&ramp(pushed, 128));
            pushed += 128;
        };
        // Orders: in, out | out, in | in, in, out, out | out, out, in, in (net one block each).
        let pattern: &[&[bool]] = &[&[true, false], &[false, true], &[true, true, false, false], &[false, true, true, false]];
        push(&feed);
        feed.pop_into(&mut out, 128);
        got.extend_from_slice(&out[0]);
        for k in 0..400 {
            for &is_in in pattern[k % pattern.len()] {
                if is_in {
                    push(&feed);
                } else {
                    feed.pop_into(&mut out, 128);
                    got.extend_from_slice(&out[0]);
                }
            }
        }
        assert_eq!(feed.extra_latency(), 128);
        let start = got.iter().position(|x| *x != 0.0).unwrap();
        let tail = &got[start..];
        assert!(tail.iter().all(|x| *x != 0.0), "no gap once primed");
        assert!(tail.windows(2).all(|w| w[1] == w[0] + 1.0), "every frame, in order");
    }

    #[test]
    fn push_overflows_are_counted() {
        let feed = InputFeed::fixed(1, 4);
        for _ in 0..10 {
            feed.push(&[0.5; 4]);
        }
        assert!(feed.overflows() > 0, "the ring (32 frames) overflowed");
    }

    #[test]
    fn the_drift_target_follows_smaller_callbacks() {
        let feed = InputFeed::drift(1, 48_000, 48_000, 512);
        feed.push(&vec![0.0; 4096]);
        for _ in 0..200 {
            feed.push(&[0.0; 480]);
        }
        assert_eq!(feed.block_size(), 480, "one big callback is not the target for good");
    }

    #[test]
    fn the_drift_target_covers_the_output_callback() {
        // 128-frame input callbacks, 1024-frame output callbacks, one rate: eight pushes, then a
        // callback half an input period after the last. A target of two input callbacks (256)
        // would run dry every callback.
        let feed = InputFeed::drift(1, 48_000, 48_000, 1024);
        let mut out = planar(1, 1024);
        let period = 128.0 / 48_000.0;
        let ns = |t: f64| (t * 1e9) as u64 + 1;
        for cb in 0..400 {
            for k in 0..8 {
                feed.push_at(&[0.5; 128], ns((cb * 8 + k) as f64 * period));
            }
            feed.begin(1024);
            feed.pop_at(&mut out, 1024, ns((cb * 8 + 7) as f64 * period + period / 2.0));
            if cb > 2 {
                assert!(out[0].iter().all(|x| *x == 0.5), "callback {cb} ran dry");
            }
        }
        assert_eq!(feed.underflows(), 0);
        assert!(feed.extra_latency() >= 1024, "the latency covers an output callback: {}", feed.extra_latency());
    }

    #[test]
    fn drift_feed_holds_latency_within_a_millisecond() {
        // Two clocks, input 100 ppm fast, 480-frame callbacks, five simulated minutes, with
        // push and pop timestamps. The input-to-output latency stays within 1 ms.
        let (rate, block, ppm) = (48_000.0f64, 480usize, 100.0);
        let true_in = rate * (1.0 + ppm * 1e-6);
        let feed = InputFeed::drift(1, 48_000, 48_000, 512);
        let mut buf = planar(1, block);
        let input = vec![0.25f32; block];
        let (mut ins, mut outs) = (0usize, 0usize);
        let (mut lo, mut hi, mut worst_adjust) = (f64::MAX, f64::MIN, 0.0f64);
        let ns = |t: f64| (t * 1e9) as u64 + 1;
        while (outs * block) as f64 / rate < 300.0 {
            let t_in = ((ins + 1) * block) as f64 / true_in;
            let t_out = (outs * block) as f64 / rate;
            if t_in <= t_out {
                feed.push_at(&input, ns(t_in));
                ins += 1;
            } else {
                if t_out >= 30.0 {
                    // The frame about to play was captured at `read / true_in`.
                    let latency = t_out - feed.read_position() / true_in;
                    lo = lo.min(latency);
                    hi = hi.max(latency);
                    worst_adjust = worst_adjust.max((feed.adjust() - ppm * 1e-6).abs());
                }
                feed.pop_at(&mut buf, block, ns(t_out));
                outs += 1;
            }
        }
        assert_eq!(feed.underflows(), 0);
        assert!(hi - lo < 1e-3, "latency wandered {:.2} ms ({:.2}..{:.2})", (hi - lo) * 1e3, lo * 1e3, hi * 1e3);
        assert!(worst_adjust < 20e-6, "pitch off by {:.1} ppm", worst_adjust * 1e6);
    }
    #[test]
    fn the_drift_latency_is_the_averaged_fill_not_the_target() {
        // Steady 480-frame callbacks on one rate, then the input bunches up: three extra blocks
        // arrive at once (under the too-full limit, so nothing is skipped). The steering takes
        // many seconds to drain them; meanwhile the input really is ~1440 frames later, and the
        // reported latency says so instead of repeating the target (960).
        let block = 480usize;
        let feed = InputFeed::drift(1, 48_000, 48_000, 512);
        let mut buf = planar(1, block);
        let period = block as f64 / 48_000.0;
        let ns = |t: f64| (t * 1e9) as u64 + 1;
        let mut t = 0.0;
        let mut step = |feed: &InputFeed, t: &mut f64, extra_pushes: usize| {
            for _ in 0..1 + extra_pushes {
                feed.push_at(&[0.25; 480], ns(*t));
            }
            feed.pop_at(&mut buf, block, ns(*t + period / 2.0));
            *t += period;
        };
        for _ in 0..500 {
            step(&feed, &mut t, 0);
        }
        let settled = feed.extra_latency();
        assert!(settled.abs_diff(960) < 100, "settled at the target fill: {settled}");
        step(&feed, &mut t, 3);
        for _ in 0..300 {
            step(&feed, &mut t, 0);
        }
        // The real latency at the last pop: what was queued before it (one block more than now),
        // plus the input on its way (half a period: pops come half a period after pushes). The
        // average lags the slow drain by about its time constant (0.5 s × ~96 frames/s).
        let real = (feed.queued() + 480 + 240) as f64;
        let extra = f64::from(feed.extra_latency());
        assert!(extra > 1900.0, "the reported latency follows the fill: {extra}");
        assert!((extra - real).abs() < 96.0, "within 2 ms of the real latency: {extra} vs {real}");
        assert_eq!(feed.underflows(), 0);
    }

    #[test]
    fn the_input_callback_latency_is_kept_in_the_feed() {
        let feed = InputFeed::fixed(1, 128);
        assert_eq!(feed.input_latency(), std::time::Duration::ZERO);
        feed.set_input_latency(std::time::Duration::from_micros(2667));
        assert_eq!(feed.input_latency(), std::time::Duration::from_micros(2667));
    }
}
