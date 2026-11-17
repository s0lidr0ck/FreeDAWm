//! The block exchange over shared memory: what the host's audio thread and the helper write,
//! where, and in which order. Safe code over the atomic words of [`crate::shm`] (or of any
//! `[AtomicU32]`, which is how the tests stand two threads in for the two processes).
//!
//! **Layout** (words): a header (magic, channels, block capacity, the host's sequence number, the
//! helper's, the frames and the events of the block in flight), an event area of [`EVENT_CAP`]
//! events of four words, then the input block and the output block, planar `f32` bits,
//! `channels × max` each.
//!
//! **Realtime, never waiting** ([`Host::process`]): the audio thread collects the block it sent
//! last time if the helper has finished it, sends this one if the helper is free (events, input,
//! then its sequence number, released), and plays from a ring primed with one block of silence,
//! so the bridge adds exactly `max` frames of latency. A block the helper has not finished in time
//! is replaced, in its place in the ring, by its own input (an effect: passed through) or by
//! silence (an instrument), and counted ([`Host::overruns`]); its late output is dropped. So every
//! block sent or not contributes its frames exactly once and in order, and the latency never moves.
//!
//! **Offline** ([`Host::process_waiting`]): a bounce or a freeze waits for each block, with no
//! added latency.

use std::sync::atomic::{AtomicU32, Ordering};

const MAGIC: u32 = 0x4644_4231;
const W_MAGIC: usize = 0;
const W_CHANNELS: usize = 1;
const W_MAX: usize = 2;
const W_SEQ_HOST: usize = 3;
const W_SEQ_HELPER: usize = 4;
const W_FRAMES: usize = 5;
const W_EVENTS: usize = 6;
const HEADER: usize = 16;
/// Events one block carries at most; more wait for the next block.
pub const EVENT_CAP: usize = 512;
/// The most channels a bridged plug-in is given.
pub const MAX_CHANNELS: usize = 16;
/// The largest block.
pub const MAX_FRAMES: usize = 8192;

const EV_PARAM: u32 = 1;
const EV_MIDI: u32 = 2;
const EV_NOTES_OFF: u32 = 3;
const EV_RESET: u32 = 4;

/// What the host tells the plug-in along with a block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// Parameter `index` of the plug-in's parameter table set to `value`.
    Param {
        index: u32,
        value: f32,
    },
    /// A MIDI 1.0 channel message at a frame of the block.
    Midi {
        offset: u32,
        msg: [u8; 3],
    },
    AllNotesOff,
    Reset,
}

impl Event {
    fn encode(self) -> [u32; 4] {
        match self {
            Event::Param { index, value } => [EV_PARAM, 0, index, value.to_bits()],
            Event::Midi { offset, msg: [a, b, c] } => [EV_MIDI, offset, u32::from_le_bytes([a, b, c, 0]), 0],
            Event::AllNotesOff => [EV_NOTES_OFF, 0, 0, 0],
            Event::Reset => [EV_RESET, 0, 0, 0],
        }
    }

    fn decode(w: [u32; 4]) -> Option<Event> {
        let [kind, offset, a, b] = w;
        Some(match kind {
            EV_PARAM => Event::Param { index: a, value: f32::from_bits(b) },
            EV_MIDI => {
                let [x, y, z, _] = a.to_le_bytes();
                Event::Midi { offset, msg: [x, y, z] }
            }
            EV_NOTES_OFF => Event::AllNotesOff,
            EV_RESET => Event::Reset,
            _ => return None,
        })
    }
}

/// The words an exchange of `channels` × `max` frames needs.
pub fn words_for(channels: usize, max: usize) -> usize {
    HEADER + 4 * EVENT_CAP + 2 * channels.clamp(1, MAX_CHANNELS) * max.clamp(1, MAX_FRAMES)
}

/// Where a block's channel `c` starts (`out`: the output block).
fn audio_at(channels: usize, max: usize, out: bool, c: usize) -> usize {
    HEADER + 4 * EVENT_CAP + if out { channels * max } else { 0 } + c * max
}

/// Writes the header of a fresh exchange (the host, before the helper maps it).
pub fn init(w: &[AtomicU32], channels: usize, max: usize) -> bool {
    let (channels, max) = (channels.clamp(1, MAX_CHANNELS), max.clamp(1, MAX_FRAMES));
    if w.len() < words_for(channels, max) {
        return false;
    }
    for (i, v) in [(W_CHANNELS, channels), (W_MAX, max)] {
        if let Some(x) = w.get(i) {
            x.store(u32::try_from(v).unwrap_or(0), Ordering::Relaxed);
        }
    }
    if let Some(m) = w.get(W_MAGIC) {
        m.store(MAGIC, Ordering::Release);
    }
    true
}

/// The shape an exchange says it has, when its header is valid and it is that large.
pub fn shape(w: &[AtomicU32]) -> Option<(usize, usize)> {
    let get = |i: usize| w.get(i).map(|x| x.load(Ordering::Acquire));
    if get(W_MAGIC)? != MAGIC {
        return None;
    }
    let channels = usize::try_from(get(W_CHANNELS)?).ok().filter(|c| (1..=MAX_CHANNELS).contains(c))?;
    let max = usize::try_from(get(W_MAX)?).ok().filter(|m| (1..=MAX_FRAMES).contains(m))?;
    (w.len() >= words_for(channels, max)).then_some((channels, max))
}

fn load(w: &[AtomicU32], i: usize) -> u32 {
    w.get(i).map_or(0, |x| x.load(Ordering::Acquire))
}

fn store(w: &[AtomicU32], i: usize, v: u32) {
    if let Some(x) = w.get(i) {
        x.store(v, Ordering::Release);
    }
}

/// The host's side, on the audio thread. Everything is allocated in [`Host::new`].
pub struct Host {
    channels: usize,
    max: usize,
    /// The last block sent (its sequence number).
    seq: u32,
    /// Frames of the block in flight, if one is.
    in_flight: Option<usize>,
    /// The block in flight's input, its stand-in if it comes back late.
    dry: Vec<Vec<f32>>,
    /// What plays: per channel, `cap` frames from `read`, `fill` of them queued.
    ring: Vec<Vec<f32>>,
    read: usize,
    fill: usize,
    events: Vec<Event>,
    /// An instrument's late block is silence, an effect's is its input.
    instrument: bool,
    overruns: u64,
    late: u64,
}

impl Host {
    /// A host side for `channels` × `max`, its ring primed with one block of silence.
    pub fn new(channels: usize, max: usize, instrument: bool) -> Host {
        let (channels, max) = (channels.clamp(1, MAX_CHANNELS), max.clamp(1, MAX_FRAMES));
        let cap = 4 * max;
        Host {
            channels,
            max,
            seq: 0,
            in_flight: None,
            dry: vec![vec![0.0; max]; channels],
            ring: vec![vec![0.0; cap]; channels],
            read: 0,
            fill: max,
            events: Vec::with_capacity(EVENT_CAP),
            instrument,
            overruns: 0,
            late: 0,
        }
    }

    /// The latency the exchange adds: one block.
    pub fn latency(&self) -> usize {
        self.max
    }

    /// Blocks not back in time so far.
    pub fn overruns(&self) -> u64 {
        self.overruns
    }

    /// Frames stood in for since the helper last kept up.
    pub fn late_frames(&self) -> u64 {
        self.late
    }

    /// Queues an event for the next block sent; `false` when the queue is full (it is dropped).
    pub fn queue(&mut self, e: Event) -> bool {
        if self.events.len() >= EVENT_CAP {
            return false;
        }
        self.events.push(e);
        true
    }

    /// One realtime block (see the module docs): never waits, never allocates.
    pub fn process(&mut self, w: &[AtomicU32], io: &mut [Vec<f32>], frames: usize) {
        let frames = frames.min(self.max).min(io.iter().map(Vec::len).min().unwrap_or(0));
        let idle = load(w, W_SEQ_HELPER) == self.seq;
        if let Some(f) = self.in_flight.take() {
            if idle {
                self.late = 0;
                for c in 0..self.channels {
                    let at = audio_at(self.channels, self.max, true, c);
                    self.push_with(c, f, |i| f32::from_bits(load(w, at + i)));
                }
            } else {
                self.stand_in_for_dry(f);
            }
            self.fill = self.fill.saturating_add(f).min(self.ring_cap());
        }
        if idle && shape(w) == Some((self.channels, self.max)) {
            self.send(w, io, frames);
        } else {
            // The helper is still on an older block: this one is stood in for now, in order.
            for c in 0..self.channels {
                let src = io.get(c);
                let instrument = self.instrument;
                self.push_with(c, frames, |i| if instrument { 0.0 } else { src.and_then(|s| s.get(i)).copied().unwrap_or(0.0) });
            }
            self.fill = self.fill.saturating_add(frames).min(self.ring_cap());
            self.overruns = self.overruns.saturating_add(1);
            self.late = self.late.saturating_add(frames as u64);
        }
        self.pop(io, frames);
    }

    /// One block for an offline render: sent, then waited for while `wait()` says to (it may
    /// sleep or give up), and played at once, with no added latency. A block not back is passed
    /// through (an effect) or silent (an instrument), and counted.
    pub fn process_waiting(&mut self, w: &[AtomicU32], io: &mut [Vec<f32>], frames: usize, mut wait: impl FnMut() -> bool) {
        let frames = frames.min(self.max).min(io.iter().map(Vec::len).min().unwrap_or(0));
        while load(w, W_SEQ_HELPER) != self.seq {
            if !wait() {
                self.stand_in(io, frames);
                return;
            }
        }
        if shape(w) != Some((self.channels, self.max)) {
            self.stand_in(io, frames);
            return;
        }
        self.send(w, io, frames);
        self.in_flight = None;
        while load(w, W_SEQ_HELPER) != self.seq {
            if !wait() {
                self.stand_in(io, frames);
                return;
            }
        }
        self.late = 0;
        for (c, ch) in io.iter_mut().enumerate().take(self.channels) {
            let at = audio_at(self.channels, self.max, true, c);
            for (i, x) in ch.iter_mut().take(frames).enumerate() {
                *x = f32::from_bits(load(w, at + i));
            }
        }
    }

    fn stand_in(&mut self, io: &mut [Vec<f32>], frames: usize) {
        if self.instrument {
            io.iter_mut().take(self.channels).for_each(|c| c.iter_mut().take(frames).for_each(|x| *x = 0.0));
        }
        self.overruns = self.overruns.saturating_add(1);
        self.late = self.late.saturating_add(frames as u64);
    }

    fn ring_cap(&self) -> usize {
        self.ring.first().map_or(0, Vec::len)
    }

    /// The in-flight block's stand-in, `f` frames, into the ring.
    fn stand_in_for_dry(&mut self, f: usize) {
        let (read, fill, instrument) = (self.read, self.fill, self.instrument);
        for (ring, dry) in self.ring.iter_mut().zip(&self.dry) {
            push_into(ring, read, fill, f, |i| if instrument { 0.0 } else { dry.get(i).copied().unwrap_or(0.0) });
        }
        self.overruns = self.overruns.saturating_add(1);
        self.late = self.late.saturating_add(f as u64);
    }

    /// Writes `n` frames of channel `c` after what is queued (the fill is moved by the caller).
    fn push_with(&mut self, c: usize, n: usize, sample: impl Fn(usize) -> f32) {
        let (read, fill) = (self.read, self.fill);
        if let Some(ring) = self.ring.get_mut(c) {
            push_into(ring, read, fill, n, sample);
        }
    }

    /// Plays `frames` from the ring into `io` (silence where it runs short).
    fn pop(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let cap = self.ring_cap().max(1);
        let n = frames.min(self.fill);
        for (c, ch) in io.iter_mut().enumerate().take(self.channels) {
            let ring = self.ring.get(c);
            for (i, x) in ch.iter_mut().take(frames).enumerate() {
                *x = if i < n { ring.and_then(|r| r.get((self.read + i) % cap)).copied().unwrap_or(0.0) } else { 0.0 };
            }
        }
        self.read = (self.read + n) % cap;
        self.fill -= n;
    }

    /// Sends `io`'s block: events, input, frames, then the sequence number (released).
    fn send(&mut self, w: &[AtomicU32], io: &[Vec<f32>], frames: usize) {
        let n = self.events.len().min(EVENT_CAP);
        for (k, e) in self.events.iter().take(n).enumerate() {
            for (j, v) in e.encode().into_iter().enumerate() {
                if let Some(x) = w.get(HEADER + 4 * k + j) {
                    x.store(v, Ordering::Relaxed);
                }
            }
        }
        self.events.clear();
        for c in 0..self.channels {
            let at = audio_at(self.channels, self.max, false, c);
            let src = io.get(c);
            for i in 0..frames {
                let v = src.and_then(|s| s.get(i)).copied().unwrap_or(0.0);
                if let Some(x) = w.get(at + i) {
                    x.store(v.to_bits(), Ordering::Relaxed);
                }
                if let Some(d) = self.dry.get_mut(c).and_then(|d| d.get_mut(i)) {
                    *d = v;
                }
            }
        }
        if let Some(x) = w.get(W_FRAMES) {
            x.store(u32::try_from(frames).unwrap_or(0), Ordering::Relaxed);
        }
        if let Some(x) = w.get(W_EVENTS) {
            x.store(u32::try_from(n).unwrap_or(0), Ordering::Relaxed);
        }
        self.seq = self.seq.wrapping_add(1);
        store(w, W_SEQ_HOST, self.seq);
        self.in_flight = Some(frames);
    }
}

/// Writes `n` samples into `ring` after the `fill` queued from `read`, as far as there is room.
fn push_into(ring: &mut [f32], read: usize, fill: usize, n: usize, sample: impl Fn(usize) -> f32) {
    let cap = ring.len().max(1);
    for i in 0..n.min(ring.len().saturating_sub(fill)) {
        if let Some(x) = ring.get_mut((read + fill + i) % cap) {
            *x = sample(i);
        }
    }
}

/// The helper's side: waits for blocks and answers them.
pub struct Helper {
    channels: usize,
    max: usize,
    seen: u32,
    io: Vec<Vec<f32>>,
    events: Vec<Event>,
}

impl Helper {
    /// The helper side of an exchange the host made (`None`: its header is not valid).
    pub fn new(w: &[AtomicU32]) -> Option<Helper> {
        let (channels, max) = shape(w)?;
        Some(Helper { channels, max, seen: load(w, W_SEQ_HELPER), io: vec![vec![0.0; max]; channels], events: Vec::with_capacity(EVENT_CAP) })
    }

    /// Whether the host has sent a block not answered yet.
    pub fn pending(&self, w: &[AtomicU32]) -> bool {
        load(w, W_SEQ_HOST) != self.seen
    }

    /// Answers the block the host sent, if it sent one: `run` gets its events, then processes
    /// its audio in place; the result goes back with the helper's sequence number (released).
    pub fn serve(&mut self, w: &[AtomicU32], run: impl FnOnce(&[Event], &mut [Vec<f32>], usize)) -> bool {
        let seq = load(w, W_SEQ_HOST);
        if seq == self.seen {
            return false;
        }
        let frames = usize::try_from(load(w, W_FRAMES)).unwrap_or(0).min(self.max);
        let n = usize::try_from(load(w, W_EVENTS)).unwrap_or(0).min(EVENT_CAP);
        self.events.clear();
        for k in 0..n {
            let mut e = [0u32; 4];
            for (j, v) in e.iter_mut().enumerate() {
                *v = w.get(HEADER + 4 * k + j).map_or(0, |x| x.load(Ordering::Relaxed));
            }
            if let Some(e) = Event::decode(e) {
                self.events.push(e);
            }
        }
        for (c, ch) in self.io.iter_mut().enumerate() {
            let at = audio_at(self.channels, self.max, false, c);
            for (i, x) in ch.iter_mut().take(frames).enumerate() {
                *x = f32::from_bits(w.get(at + i).map_or(0, |v| v.load(Ordering::Relaxed)));
            }
        }
        run(&self.events, &mut self.io, frames);
        for (c, ch) in self.io.iter().enumerate() {
            let at = audio_at(self.channels, self.max, true, c);
            for (i, x) in ch.iter().take(frames).enumerate() {
                if let Some(v) = w.get(at + i) {
                    v.store(x.to_bits(), Ordering::Relaxed);
                }
            }
        }
        self.seen = seq;
        store(w, W_SEQ_HELPER, seq);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    fn exchange(channels: usize, max: usize) -> Arc<Vec<AtomicU32>> {
        let w: Vec<AtomicU32> = (0..words_for(channels, max)).map(|_| AtomicU32::new(0)).collect();
        assert!(init(&w, channels, max));
        Arc::new(w)
    }

    /// Block `b` of a stereo signal, every sample distinct.
    fn block(b: usize, frames: usize) -> Vec<Vec<f32>> {
        (0..2).map(|c| (0..frames).map(|i| (b * 1000 + i * 2 + c + 1) as f32).collect()).collect()
    }

    #[test]
    fn events_round_trip() {
        for e in [Event::Param { index: 7, value: -0.25 }, Event::Midi { offset: 31, msg: [0x91, 60, 100] }, Event::AllNotesOff, Event::Reset] {
            assert_eq!(Event::decode(e.encode()), Some(e));
        }
        assert_eq!(Event::decode([99, 0, 0, 0]), None);
    }

    #[test]
    fn a_helper_that_keeps_up_plays_every_block_one_block_late() {
        // Two threads stand in for the two processes: the helper doubles each sample.
        let w = exchange(2, 64);
        let stop = Arc::new(AtomicBool::new(false));
        let (hw, hs) = (Arc::clone(&w), Arc::clone(&stop));
        let helper = std::thread::spawn(move || {
            let mut h = Helper::new(&hw).unwrap();
            let mut got = Vec::new();
            while !hs.load(Ordering::Relaxed) || h.pending(&hw) {
                h.serve(&hw, |events, io, n| {
                    got.extend_from_slice(events);
                    io.iter_mut().for_each(|c| c.iter_mut().take(n).for_each(|x| *x *= 2.0));
                });
                std::hint::spin_loop();
            }
            got
        });
        let mut host = Host::new(2, 64, false);
        assert!(host.queue(Event::Param { index: 1, value: 0.5 }));
        let (mut sent, mut heard) = (vec![Vec::new(); 2], vec![Vec::new(); 2]);
        for b in 0..40 {
            let mut io = block(b, 64);
            (0..2).for_each(|c| sent[c].extend_from_slice(&io[c]));
            host.process(&w, &mut io, 64);
            (0..2).for_each(|c| heard[c].extend_from_slice(&io[c]));
            // Give the helper the rest of the "callback period".
            while load(&w, W_SEQ_HELPER) != host.seq {
                std::hint::spin_loop();
            }
        }
        stop.store(true, Ordering::Relaxed);
        assert_eq!(helper.join().unwrap(), vec![Event::Param { index: 1, value: 0.5 }], "the event went with the first block");
        assert_eq!(host.overruns(), 0);
        for c in 0..2 {
            assert!(heard[c][..64].iter().all(|x| *x == 0.0), "one block of latency");
            let want: Vec<f32> = sent[c][..39 * 64].iter().map(|x| x * 2.0).collect();
            assert_eq!(heard[c][64..], want[..]);
        }
    }

    #[test]
    fn a_block_not_back_in_time_is_its_own_input_and_the_timing_holds() {
        // No helper at all (it hung): an effect passes its input through, one block late, as if
        // bypassed; an instrument is silent. Nothing waits.
        let w = exchange(2, 32);
        for instrument in [false, true] {
            let mut host = Host::new(2, 32, instrument);
            let mut heard = Vec::new();
            let mut sent = Vec::new();
            for b in 0..10 {
                let mut io = block(b, 32);
                sent.extend_from_slice(&io[0]);
                host.process(&w, &mut io, 32);
                heard.extend_from_slice(&io[0]);
            }
            let want: Vec<f32> = if instrument { vec![0.0; 9 * 32] } else { sent[..9 * 32].to_vec() };
            assert_eq!(heard[32..], want[..], "instrument {instrument}");
            assert_eq!(host.overruns(), 10);
            assert_eq!(host.late_frames(), 10 * 32);
        }
    }

    #[test]
    fn a_late_block_coming_back_after_all_is_dropped_and_the_next_one_plays() {
        let w = exchange(2, 16);
        let mut host = Host::new(2, 16, false);
        let mut helper = Helper::new(&w).unwrap();
        let triple = |_: &[Event], io: &mut [Vec<f32>], n: usize| io.iter_mut().for_each(|c| c.iter_mut().take(n).for_each(|x| *x *= 3.0));
        let mut out = Vec::new();
        let mut go = |host: &mut Host, b: usize| {
            let mut io = block(b, 16);
            host.process(&w, &mut io, 16);
            out.push(io[0].clone());
        };
        go(&mut host, 0); // block 0 sent
        go(&mut host, 1); // 0 late: stood in for by its input; 1 not sent (the helper is busy)
        helper.serve(&w, triple); // 0 comes back late
        go(&mut host, 2); // 0's output is dropped; 2 is sent
        helper.serve(&w, triple);
        go(&mut host, 3); // 2 is back
        assert_eq!(out[0], vec![0.0; 16]);
        assert_eq!(out[1], block(0, 16)[0], "block 0, passed through");
        assert_eq!(out[2], block(1, 16)[0], "block 1, passed through");
        assert_eq!(out[3], block(2, 16)[0].iter().map(|x| x * 3.0).collect::<Vec<_>>(), "block 2, processed, in its place");
        assert_eq!(host.overruns(), 2);
        assert_eq!(host.late_frames(), 0, "kept up again");
    }

    #[test]
    fn offline_waits_for_each_block_with_no_latency() {
        let w = exchange(2, 64);
        let hw = Arc::clone(&w);
        let helper = std::thread::spawn(move || {
            let mut h = Helper::new(&hw).unwrap();
            let mut served = 0;
            while served < 5 {
                if h.serve(&hw, |_, io, n| io.iter_mut().for_each(|c| c.iter_mut().take(n).for_each(|x| *x = -*x))) {
                    served += 1;
                }
                std::thread::yield_now();
            }
        });
        let mut host = Host::new(2, 64, false);
        for b in 0..5 {
            let mut io = block(b, 48);
            host.process_waiting(&w, &mut io, 48, || {
                std::thread::yield_now();
                true
            });
            assert_eq!(io[1], block(b, 48)[1].iter().map(|x| -x).collect::<Vec<_>>());
        }
        helper.join().unwrap();
        // With nobody answering, it gives up when told to and passes the block through.
        let mut io = block(9, 48);
        let mut tries = 0;
        host.process_waiting(&w, &mut io, 48, || {
            tries += 1;
            tries < 3
        });
        assert_eq!(io, block(9, 48));
        assert_eq!(host.overruns(), 1);
    }

    #[test]
    fn hostile_headers_are_refused() {
        let w: Vec<AtomicU32> = (0..words_for(2, 64)).map(|_| AtomicU32::new(0)).collect();
        assert!(Helper::new(&w).is_none(), "no magic");
        assert!(init(&w, 2, 64));
        w[W_MAX].store(1 << 30, Ordering::Relaxed);
        assert!(shape(&w).is_none(), "a block larger than the file");
        assert!(!init(&w[..10], 2, 64), "too small");
    }
}
