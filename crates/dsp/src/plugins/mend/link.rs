//! What Mend's audio side and its worker share: two lock-free queues of samples (one writer and
//! one reader each), the stream's restart points and the worker's state. Nothing here allocates
//! or locks after [`Link::new`], so the audio thread can use it freely.
//!
//! Positions count frames. The audio side numbers the frames it has queued (`sent`); a restart
//! ([`Link::restart`]) names the frame a new stream begins at, and the worker forgets everything
//! before it. The worker numbers the frames it has answered with (`made`) and says where the
//! answers of each stream begin ([`Link::answer_from`]), so the audio side can drop what belongs
//! to a stream it has left.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// The model's sample rate.
pub const MODEL_RATE: u32 = 48_000;
/// The model's STFT hop at [`MODEL_RATE`].
pub const HOP: usize = 2048;
/// How many seconds of sound each queue holds.
const QUEUE_SECS: usize = 8;

/// How the stream is cut into pieces for the model at one session rate. Lengths are in frames at
/// the session rate; the `_hops` are the same lengths in model hops ([`HOP`] at [`MODEL_RATE`]).
/// Every length is a whole number of samples at both rates, so the pieces line up exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub rate: u32,
    /// What one pass of the model answers.
    pub block: usize,
    /// The sound before the block the model hears with it.
    pub past: usize,
    /// The sound after the block the model hears with it.
    pub look: usize,
    pub block_hops: usize,
    pub past_hops: usize,
    pub look_hops: usize,
}

const fn gcd(mut a: usize, mut b: usize) -> usize {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

impl Plan {
    /// The plan at `rate`; `None` for a rate that has no short common measure with the model's
    /// (Mend then passes the sound through).
    pub fn for_rate(rate: u32) -> Option<Plan> {
        if !(8_000..=384_000).contains(&rate) {
            return None;
        }
        let (r, m) = (rate as usize, MODEL_RATE as usize);
        let g = gcd(r, m);
        // `up` model samples last as long as `down` session samples.
        let (up, down) = (m / g, r / g);
        // The fewest hops that are a whole number of session samples.
        let unit = up / gcd(HOP, up);
        if unit > 25 {
            return None;
        }
        let round = |hops: usize| hops.div_ceil(unit) * unit;
        let (block_hops, past_hops, look_hops) = (round(25), round(40), round(17));
        let frames = |hops: usize| hops * HOP / up * down;
        Some(Plan { rate, block: frames(block_hops), past: frames(past_hops), look: frames(look_hops), block_hops, past_hops, look_hops })
    }

    /// Mend's latency: a block has to be whole and its look-ahead heard before the model starts,
    /// and the model is given one more block's time to answer.
    pub fn latency(&self) -> usize {
        2 * self.block + self.look
    }
}

/// What the worker is doing with a link ([`Link::state`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    /// No worker has picked the link up (this build has none, or it has not got to it yet).
    Waiting = 0,
    /// The model is being loaded.
    Loading = 1,
    /// The model is answering.
    Ready = 2,
    /// The model is not installed.
    NoModel = 3,
    /// The model could not be loaded or failed while running.
    Failed = 4,
}

impl State {
    pub fn from_u8(v: u8) -> State {
        match v {
            1 => State::Loading,
            2 => State::Ready,
            3 => State::NoModel,
            4 => State::Failed,
            _ => State::Waiting,
        }
    }
}

/// A queue of samples for one writer and one reader.
struct Queue {
    buf: Box<[AtomicU32]>,
    mask: usize,
    written: AtomicUsize,
    read: AtomicUsize,
}

impl Queue {
    fn new(samples: usize) -> Queue {
        let cap = samples.max(2).next_power_of_two();
        Queue { buf: (0..cap).map(|_| AtomicU32::new(0)).collect(), mask: cap - 1, written: AtomicUsize::new(0), read: AtomicUsize::new(0) }
    }

    fn len(&self) -> usize {
        self.written.load(Ordering::Acquire).wrapping_sub(self.read.load(Ordering::Acquire))
    }

    fn free(&self) -> usize {
        self.buf.len().saturating_sub(self.len())
    }

    /// Queues every sample `next` gives (the writer's side); the caller has checked the room.
    fn push(&self, count: usize, mut next: impl FnMut(usize) -> f32) {
        let w = self.written.load(Ordering::Relaxed);
        for i in 0..count {
            if let Some(cell) = self.buf.get(w.wrapping_add(i) & self.mask) {
                cell.store(next(i).to_bits(), Ordering::Relaxed);
            }
        }
        self.written.store(w.wrapping_add(count), Ordering::Release);
    }

    /// Takes up to `count` samples (the reader's side), giving each to `take`; how many it took.
    fn pop(&self, count: usize, mut take: impl FnMut(usize, f32)) -> usize {
        let n = count.min(self.len());
        let r = self.read.load(Ordering::Relaxed);
        for i in 0..n {
            if let Some(cell) = self.buf.get(r.wrapping_add(i) & self.mask) {
                take(i, f32::from_bits(cell.load(Ordering::Relaxed)));
            }
        }
        self.read.store(r.wrapping_add(n), Ordering::Release);
        n
    }
}

/// One Mend instance's connection to the worker.
pub struct Link {
    pub plan: Plan,
    /// Channels in each frame (1 or 2).
    pub channels: usize,
    to_worker: Queue,
    from_worker: Queue,
    /// The stream the audio side is on, and the frame (in `sent` frames) it began at.
    stream: AtomicU32,
    stream_from: AtomicU64,
    /// The stream the worker's newest answers belong to, and the answer frame it began at.
    answer_stream: AtomicU32,
    answer_from: AtomicU64,
    state: AtomicU8,
    /// The model's time per block against the block's own length, in thousandths.
    load: AtomicU32,
    offline: AtomicBool,
    closed: AtomicBool,
    /// The worker has lost its place in the stream and wants a new one.
    lost: AtomicBool,
}

impl std::fmt::Debug for Link {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Link").field("plan", &self.plan).field("channels", &self.channels).field("state", &self.state()).finish()
    }
}

impl Link {
    pub fn new(plan: Plan, channels: usize) -> Link {
        let channels = channels.clamp(1, 2);
        let samples = plan.rate as usize * QUEUE_SECS * channels;
        Link {
            plan,
            channels,
            to_worker: Queue::new(samples),
            from_worker: Queue::new(samples),
            stream: AtomicU32::new(0),
            stream_from: AtomicU64::new(0),
            answer_stream: AtomicU32::new(0),
            answer_from: AtomicU64::new(0),
            state: AtomicU8::new(State::Waiting as u8),
            load: AtomicU32::new(0),
            offline: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            lost: AtomicBool::new(false),
        }
    }

    pub fn state(&self) -> State {
        State::from_u8(self.state.load(Ordering::Relaxed))
    }

    pub fn set_state(&self, s: State) {
        self.state.store(s as u8, Ordering::Relaxed);
    }

    /// The model's time per block as a share of the block's length (1.0: only just keeping up).
    pub fn load(&self) -> f32 {
        self.load.load(Ordering::Relaxed) as f32 / 1000.0
    }

    pub fn set_load(&self, share: f32) {
        self.load.store((share.clamp(0.0, 100.0) * 1000.0) as u32, Ordering::Relaxed);
    }

    /// Whether the audio side is rendering offline (it waits for answers; the worker need not
    /// pace itself).
    pub fn offline(&self) -> bool {
        self.offline.load(Ordering::Relaxed)
    }

    pub fn set_offline(&self, on: bool) {
        self.offline.store(on, Ordering::Relaxed);
    }

    /// The audio side is gone: the worker lets the link go.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }

    pub fn closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    // ---- The audio side.

    /// Whether the worker has asked for a new stream since the last call.
    pub fn take_lost(&self) -> bool {
        self.lost.swap(false, Ordering::Relaxed)
    }

    /// Whether `frames` more frames fit in the queue to the worker.
    pub fn room_for(&self, frames: usize) -> bool {
        self.to_worker.free() >= frames * self.channels
    }

    /// Queues `frames` frames of `io` from `start` for the worker (check [`Link::room_for`]
    /// first). A channel `io` lacks repeats the first.
    pub fn send(&self, io: &[Vec<f32>], start: usize, frames: usize) {
        let ch = self.channels;
        self.to_worker.push(frames * ch, |i| {
            let (f, c) = (start + i / ch, i % ch);
            io.get(c).or(io.first()).and_then(|b| b.get(f)).copied().unwrap_or(0.0)
        });
    }

    /// Starts a new stream at frame `from` of everything sent so far.
    pub fn restart(&self, stream: u32, from: u64) {
        self.stream_from.store(from, Ordering::Release);
        self.stream.store(stream, Ordering::Release);
    }

    /// The stream the worker's answers are on and the answer frame it began at.
    pub fn answer_from(&self) -> (u32, u64) {
        let s = self.answer_stream.load(Ordering::Acquire);
        (s, self.answer_from.load(Ordering::Acquire))
    }

    /// Answer frames waiting.
    pub fn answers(&self) -> usize {
        self.from_worker.len() / self.channels
    }

    /// Drops up to `frames` waiting answer frames; how many it dropped.
    pub fn skip_answers(&self, frames: usize) -> usize {
        self.from_worker.pop(frames * self.channels, |_, _| {}) / self.channels
    }

    /// Takes up to `frames` answer frames into `out` (interleaved); how many frames it took.
    pub fn take_answers(&self, out: &mut [f32], frames: usize) -> usize {
        let ch = self.channels;
        let want = frames.min(out.len() / ch).min(self.answers());
        self.from_worker.pop(want * ch, |i, v| {
            if let Some(o) = out.get_mut(i) {
                *o = v;
            }
        }) / ch
    }

    // ---- The worker's side.

    /// Asks the audio side to start a new stream: the worker dropped frames of this one (no
    /// model yet, or it fell too far behind), so it cannot answer the rest of it.
    pub fn ask_restart(&self) {
        self.lost.store(true, Ordering::Relaxed);
    }

    /// The audio side's stream and the sent frame it began at.
    pub fn stream(&self) -> (u32, u64) {
        loop {
            let s = self.stream.load(Ordering::Acquire);
            let from = self.stream_from.load(Ordering::Acquire);
            if self.stream.load(Ordering::Acquire) == s {
                return (s, from);
            }
        }
    }

    /// Takes every waiting sent frame, appending it to `out` (interleaved); how many frames.
    pub fn receive(&self, out: &mut Vec<f32>) -> usize {
        let n = self.to_worker.len() / self.channels * self.channels;
        out.reserve(n);
        self.to_worker.pop(n, |_, v| out.push(v)) / self.channels
    }

    /// Whether `frames` answer frames fit in the queue back.
    pub fn room_for_answer(&self, frames: usize) -> bool {
        self.from_worker.free() >= frames * self.channels
    }

    /// Says that answers from frame `from` (of everything answered so far) are on `stream`.
    pub fn answer_starts(&self, stream: u32, from: u64) {
        self.answer_from.store(from, Ordering::Release);
        self.answer_stream.store(stream, Ordering::Release);
    }

    /// Queues answer frames (interleaved; check [`Link::room_for_answer`] first).
    pub fn answer(&self, frames: &[f32]) {
        self.from_worker.push(frames.len(), |i| frames.get(i).copied().unwrap_or(0.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_line_up_at_both_rates() {
        for rate in [44_100u32, 48_000, 88_200, 96_000, 176_400, 192_000, 32_000, 24_000] {
            let p = Plan::for_rate(rate).unwrap_or_else(|| panic!("{rate}"));
            for (frames, hops) in [(p.block, p.block_hops), (p.past, p.past_hops), (p.look, p.look_hops)] {
                // The same length of time at both rates, exactly.
                assert_eq!(frames as u64 * u64::from(MODEL_RATE), (hops * HOP) as u64 * u64::from(rate), "{rate}");
            }
            assert!(p.block_hops >= 25 && p.look_hops >= 17 && p.past_hops >= 40);
            let secs = p.latency() as f32 / rate as f32;
            assert!((2.5..3.6).contains(&secs), "{rate}: {secs} s");
        }
        assert_eq!(Plan::for_rate(48_000).map(|p| p.latency()), Some(137_216));
        assert!(Plan::for_rate(44_101).is_none() && Plan::for_rate(0).is_none());
    }

    #[test]
    fn queues_carry_frames_in_order_and_wrap() {
        let plan = Plan::for_rate(8_000).unwrap();
        let link = Link::new(plan, 2);
        let mut seen = Vec::new();
        let mut n = 0.0f32;
        for round in 0..200 {
            let frames = 1 + (round * 37) % 1500;
            assert!(link.room_for(frames));
            let io: Vec<Vec<f32>> = (0..2).map(|c| (0..frames).map(|i| n + i as f32 + c as f32 * 0.5).collect()).collect();
            link.send(&io, 0, frames);
            let before = seen.len();
            assert_eq!(link.receive(&mut seen), frames);
            for (i, pair) in seen[before..].chunks(2).enumerate() {
                assert_eq!(pair, [n + i as f32, n + i as f32 + 0.5]);
            }
            n += frames as f32;
        }
        // The way back, with skipping.
        link.answer(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(link.answers(), 3);
        assert_eq!(link.skip_answers(1), 1);
        let mut out = [0.0f32; 8];
        assert_eq!(link.take_answers(&mut out, 4), 2);
        assert_eq!(out[..4], [3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn a_full_queue_says_so() {
        let plan = Plan::for_rate(8_000).unwrap();
        let link = Link::new(plan, 1);
        let io = vec![vec![0.25f32; 4096]];
        let mut sent = 0;
        while link.room_for(4096) {
            link.send(&io, 0, 4096);
            sent += 4096;
        }
        assert!(sent >= 8_000 * 8 && !link.room_for(4096));
    }
}
