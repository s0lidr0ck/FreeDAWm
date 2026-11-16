//! MIDI output to hardware: the External MIDI instrument and the rings it sends through.
//!
//! An instrument or MIDI track whose instrument is External MIDI ([`ID`], its state the output
//! port's name, its `channel` parameter 0 for "as played" or 1-16) sends what it plays (its clips,
//! its live input, the panic) to a hardware MIDI output port instead of making sound. The plugin
//! runs where any instrument runs, so clip scheduling, MIDI Thru, chasing, the transport's stops
//! and All MIDI Notes Off all reach the port as they reach a synth.
//!
//! - [`OutHub`] (one per process, [`hub`]): one lock-free bounded multi-producer queue
//!   ([`OutRing`], atomics only, preallocated, no `unsafe`) per open output port slot. Strips render
//!   in parallel, so several tracks may push into one port's ring at once. Each entry carries
//!   when it should leave (µs on [`OutHub::now_us`]'s clock).
//! - The audio host stamps each block: [`OutHub::begin_block`] gives the time the block's first
//!   frame will be heard (now, plus what is queued before it, the output latency and the mix's
//!   delay compensation), so a note leaves when the audio beside it is heard.
//! - The host's sender (`soundcraft_playback::midi_out`) opens the port, takes the slot
//!   ([`OutHub::open`]), and sends each entry at its time from its own thread; closing a port
//!   lets go of every note it left on.
//!
//! On the audio thread nothing here allocates, locks or panics: a full ring drops (and counts),
//! a port that is closed or disabled drops silently. An offline render (bounce, freeze) sends
//! nothing ([`soundcraft_dsp::Plugin::set_offline`]).

use soundcraft_dsp::{Category, ParamInfo, Plugin, PluginInfo, Taper, Unit};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

/// The External MIDI instrument's plugin id.
pub const ID: &str = "external_midi";

/// Output ports open at once.
pub const MAX_OUT_PORTS: usize = 16;

/// Messages one port's ring holds between two passes of its sender (about a millisecond).
pub const OUT_RING: usize = 4096;

/// Bounded multi-producer multi-consumer FIFO of `(stamp, packed message)` pairs (Vyukov's
/// sequence-numbered cells, with atomics for the payload so no `unsafe` is needed).
pub struct OutRing {
    cells: Box<[(AtomicUsize, AtomicU64, AtomicU64)]>,
    mask: usize,
    enqueue: AtomicUsize,
    dequeue: AtomicUsize,
    dropped: AtomicU64,
}

impl OutRing {
    pub fn new(capacity: usize) -> OutRing {
        let n = capacity.clamp(2, 1 << 20).next_power_of_two();
        OutRing {
            cells: (0..n).map(|i| (AtomicUsize::new(i), AtomicU64::new(0), AtomicU64::new(0))).collect(),
            mask: n - 1,
            enqueue: AtomicUsize::new(0),
            dequeue: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
        }
    }

    /// Pushes failed for want of room.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Queues one entry from any thread; false (and counted) when full.
    pub fn push(&self, stamp: u64, packed: u64) -> bool {
        let mut pos = self.enqueue.load(Ordering::Relaxed);
        loop {
            let Some(cell) = self.cells.get(pos & self.mask) else { return false };
            let seq = cell.0.load(Ordering::Acquire);
            let diff = seq as isize - pos as isize;
            if diff == 0 {
                match self.enqueue.compare_exchange_weak(pos, pos.wrapping_add(1), Ordering::Relaxed, Ordering::Relaxed) {
                    Ok(_) => {
                        cell.1.store(stamp, Ordering::Relaxed);
                        cell.2.store(packed, Ordering::Relaxed);
                        cell.0.store(pos.wrapping_add(1), Ordering::Release);
                        return true;
                    }
                    Err(p) => pos = p,
                }
            } else if diff < 0 {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return false;
            } else {
                pos = self.enqueue.load(Ordering::Relaxed);
            }
        }
    }

    /// Takes the oldest entry.
    pub fn pop(&self) -> Option<(u64, u64)> {
        let mut pos = self.dequeue.load(Ordering::Relaxed);
        loop {
            let cell = self.cells.get(pos & self.mask)?;
            let seq = cell.0.load(Ordering::Acquire);
            let diff = seq as isize - pos.wrapping_add(1) as isize;
            if diff == 0 {
                match self.dequeue.compare_exchange_weak(pos, pos.wrapping_add(1), Ordering::Relaxed, Ordering::Relaxed) {
                    Ok(_) => {
                        let out = (cell.1.load(Ordering::Relaxed), cell.2.load(Ordering::Relaxed));
                        cell.0.store(pos.wrapping_add(self.mask).wrapping_add(1), Ordering::Release);
                        return Some(out);
                    }
                    Err(p) => pos = p,
                }
            } else if diff < 0 {
                return None;
            } else {
                pos = self.dequeue.load(Ordering::Relaxed);
            }
        }
    }
}

/// A 3-byte message packed in a ring entry.
pub fn pack(msg: [u8; 3]) -> u64 {
    u64::from(msg[0]) | (u64::from(msg[1]) << 8) | (u64::from(msg[2]) << 16)
}

/// The message in a ring entry.
pub fn unpack(v: u64) -> [u8; 3] {
    [(v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8]
}

/// How many bytes a channel message has (2 for program change and channel pressure).
pub fn message_len(status: u8) -> usize {
    if matches!(status & 0xF0, 0xC0 | 0xD0) { 2 } else { 3 }
}

/// The identity of an output port by name (never 0, which means "no port").
pub fn port_key(name: &str) -> u64 {
    // FNV-1a.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h.max(1)
}

/// The output port slots and their rings (see the module docs).
pub struct OutHub {
    epoch: Instant,
    ids: [AtomicU64; MAX_OUT_PORTS],
    rings: [OutRing; MAX_OUT_PORTS],
    /// When the frame being rendered now will be heard (µs on [`OutHub::now_us`]'s clock), 0
    /// while no live host renders.
    base: AtomicU64,
}

static HUB: OnceLock<OutHub> = OnceLock::new();

/// The process's output hub (made on first use, off the audio thread: the plugin asks for it
/// when it is made).
pub fn hub() -> &'static OutHub {
    HUB.get_or_init(|| OutHub {
        epoch: Instant::now(),
        ids: std::array::from_fn(|_| AtomicU64::new(0)),
        rings: std::array::from_fn(|_| OutRing::new(OUT_RING)),
        base: AtomicU64::new(0),
    })
}

impl OutHub {
    /// Microseconds since the hub was made (never 0).
    pub fn now_us(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_micros()).unwrap_or(u64::MAX).max(1)
    }

    /// Takes a free slot for the port named `name` (emptied first); `None` when all are taken.
    pub fn open(&self, name: &str) -> Option<usize> {
        let key = port_key(name);
        if let Some(i) = self.slot_of(key) {
            return Some(i);
        }
        for (i, id) in self.ids.iter().enumerate() {
            if id.load(Ordering::Acquire) == 0 {
                if let Some(r) = self.rings.get(i) {
                    while r.pop().is_some() {}
                }
                if id.compare_exchange(0, key, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                    return Some(i);
                }
            }
        }
        None
    }

    /// Frees slot `slot` (what is still queued in it is the closer's to drain).
    pub fn close(&self, slot: usize) {
        if let Some(id) = self.ids.get(slot) {
            id.store(0, Ordering::Release);
        }
    }

    /// Whether any port is open.
    pub fn any_open(&self) -> bool {
        self.ids.iter().any(|i| i.load(Ordering::Relaxed) != 0)
    }

    fn slot_of(&self, key: u64) -> Option<usize> {
        self.ids.iter().position(|i| i.load(Ordering::Acquire) == key)
    }

    /// The audio host, before rendering a block: the time its first frame will be heard.
    pub fn begin_block(&self, heard_at_us: u64) {
        self.base.store(heard_at_us, Ordering::Relaxed);
    }

    /// When the frame being rendered will be heard (0: no live host).
    pub fn block_base(&self) -> u64 {
        self.base.load(Ordering::Relaxed)
    }

    /// Queues `msg` for the port `key` to leave at `stamp`; false when the port is not open or its
    /// ring is full. Any thread; never allocates.
    pub fn send(&self, key: u64, stamp: u64, msg: [u8; 3]) -> bool {
        match self.slot_of(key).and_then(|i| self.rings.get(i)) {
            Some(r) => r.push(stamp, pack(msg)),
            None => false,
        }
    }

    /// The sender: the oldest entry queued for slot `slot`.
    pub fn pop(&self, slot: usize) -> Option<(u64, [u8; 3])> {
        self.rings.get(slot)?.pop().map(|(s, p)| (s, unpack(p)))
    }

    /// Messages dropped for want of room, all ports together.
    pub fn dropped(&self) -> u64 {
        self.rings.iter().map(OutRing::dropped).sum()
    }
}

static PARAMS: [ParamInfo; 1] = [ParamInfo {
    id: "channel",
    name: "Channel",
    min: 0.0,
    max: 16.0,
    default: 0.0,
    unit: Unit::None,
    taper: Taper::Linear,
    choices: &[],
    display: Some(channel_text),
    group: "",
}];

fn channel_text(v: f32) -> String {
    if v < 0.5 { "As played".into() } else { format!("{}", v.round() as u8) }
}

pub static INFO: PluginInfo = PluginInfo {
    id: ID,
    name: "External MIDI",
    short_name: "Ext MIDI",
    category: Category::Instrument,
    params: &PARAMS,
    is_instrument: true,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// The plugin info of `id` when it is External MIDI.
pub fn plugin_info(id: &str) -> Option<&'static PluginInfo> {
    (id == ID).then_some(&INFO)
}

/// A new External MIDI instance when `id` names it.
pub fn create(id: &str) -> Option<Box<dyn Plugin>> {
    (id == ID).then(|| Box::new(ExternalMidi::new()) as Box<dyn Plugin>)
}

/// The External MIDI instrument (see the module docs).
pub struct ExternalMidi {
    hub: &'static OutHub,
    port: u64,
    /// 0: each message on its own channel; 1-16: every message on that one.
    channel: u8,
    rate: f64,
    offline: bool,
    /// Notes it left on, by channel (bit per pitch), so a stop lets go of exactly those.
    held: [[u64; 2]; 16],
    /// Channels it sent on since its last stop (bit per channel).
    used: u16,
}

impl Default for ExternalMidi {
    fn default() -> Self {
        ExternalMidi::new()
    }
}

impl ExternalMidi {
    pub fn new() -> ExternalMidi {
        ExternalMidi { hub: hub(), port: 0, channel: 0, rate: 48_000.0, offline: false, held: [[0; 2]; 16], used: 0 }
    }

    /// The port it sends to (by name; empty: none).
    pub fn set_port(&mut self, name: &str) {
        self.port = if name.is_empty() { 0 } else { port_key(name) };
    }

    fn emit(&mut self, offset: usize, msg: [u8; 3]) {
        if self.port == 0 || self.offline {
            return;
        }
        let mut msg = msg;
        if self.channel > 0 && (0x80..0xF0).contains(&msg[0]) {
            msg[0] = (msg[0] & 0xF0) | ((self.channel - 1) & 0x0F);
        }
        let ch = usize::from(msg[0] & 0x0F);
        self.used |= 1 << ch;
        let (kind, pitch) = (msg[0] & 0xF0, msg[1] & 0x7F);
        if let Some(w) = self.held.get_mut(ch).and_then(|h| h.get_mut(usize::from(pitch / 64))) {
            let bit = 1u64 << (pitch % 64);
            match kind {
                0x90 if msg[2] > 0 => *w |= bit,
                0x80 | 0x90 => *w &= !bit,
                _ => {}
            }
        }
        let base = self.hub.block_base();
        let stamp = if base == 0 { 0 } else { base.saturating_add((offset as f64 * 1e6 / self.rate.max(1.0)) as u64) };
        self.hub.send(self.port, stamp, msg);
    }
}

impl Plugin for ExternalMidi {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }

    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        if sample_rate.is_finite() && sample_rate > 0.0 {
            self.rate = f64::from(sample_rate);
        }
    }

    fn reset(&mut self) {}

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if id != "channel" {
            return false;
        }
        let v = PARAMS.first().map_or(0.0, |pi| pi.clamp(value)).round() as u8;
        if v != self.channel {
            // Notes left on the old channel would never get their note-off.
            self.all_notes_off();
            self.channel = v;
        }
        true
    }

    fn param(&self, id: &str) -> Option<f32> {
        (id == "channel").then_some(f32::from(self.channel))
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        for c in io.iter_mut() {
            c.iter_mut().take(frames).for_each(|x| *x = 0.0);
        }
    }

    fn set_offline(&mut self, offline: bool) {
        self.offline = offline;
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.emit(offset, [0x90, note & 0x7F, velocity.clamp(1, 127)]);
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.emit(offset, [0x80, note & 0x7F, 0]);
    }

    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        self.emit(offset, msg);
    }

    fn midi_raw(&mut self, offset: usize, msg: [u8; 3]) {
        self.emit(offset, msg);
    }

    /// Note-offs for every note it left on, then sustain up and All Notes Off on each channel
    /// it sent on since the last time.
    fn all_notes_off(&mut self) {
        let held = std::mem::take(&mut self.held);
        let used = std::mem::take(&mut self.used);
        if self.port == 0 || self.offline {
            return;
        }
        let base = self.hub.block_base();
        for (ch, words) in held.iter().enumerate() {
            let status = |kind: u8| kind | (ch as u8 & 0x0F);
            for (w, bits) in words.iter().enumerate() {
                for k in 0..64u8 {
                    if bits & (1u64 << k) != 0 {
                        self.hub.send(self.port, base, [status(0x80), (w as u8) * 64 + k, 0]);
                    }
                }
            }
            if used & (1 << ch) != 0 {
                self.hub.send(self.port, base, [status(0xB0), 64, 0]);
                self.hub.send(self.port, base, [status(0xB0), 123, 0]);
            }
        }
    }

    /// The port's name (UTF-8).
    fn save_state(&mut self) -> Option<Vec<u8>> {
        None
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        match std::str::from_utf8(data) {
            Ok(name) if name.len() <= 256 => {
                self.set_port(name);
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_takes_many_producers_and_keeps_each_ones_order() {
        let ring = std::sync::Arc::new(OutRing::new(1 << 14));
        let threads: Vec<_> = (0..4u64)
            .map(|t| {
                let r = std::sync::Arc::clone(&ring);
                std::thread::spawn(move || {
                    for i in 0..2000u64 {
                        assert!(r.push(t, i));
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let mut last = [None::<u64>; 4];
        let mut n = 0;
        while let Some((t, i)) = ring.pop() {
            let l = &mut last[t as usize];
            assert!(l.is_none_or(|x| x < i), "each producer's order kept");
            *l = Some(i);
            n += 1;
        }
        assert_eq!(n, 8000);
        let small = OutRing::new(2);
        assert!(small.push(1, 1) && small.push(2, 2));
        assert!(!small.push(3, 3), "full drops");
        assert_eq!(small.dropped(), 1);
    }

    #[test]
    fn external_midi_sends_to_its_port_at_the_heard_time_and_lets_go_on_panic() {
        let h = hub();
        let slot = h.open("Test Synth A").unwrap();
        let mut p = ExternalMidi::new();
        assert!(p.load_state(b"Test Synth A"));
        p.prepare(48_000.0, 512, 2);
        h.begin_block(1_000_000);
        p.note_on(480, 60, 100);
        p.midi_raw(0, [0x93, 64, 1]);
        let mut io = vec![vec![1.0f32; 512]; 2];
        p.process(&mut io, 512);
        assert!(io.iter().all(|c| c.iter().all(|x| *x == 0.0)), "it makes no sound");
        assert_eq!(h.pop(slot), Some((1_010_000, [0x90, 60, 100])), "10 ms into the block");
        assert_eq!(h.pop(slot), Some((1_000_000, [0x93, 64, 1])), "its own channel kept");
        p.all_notes_off();
        let mut sent = Vec::new();
        while let Some((_, m)) = h.pop(slot) {
            sent.push(m);
        }
        assert_eq!(sent[0], [0x80, 60, 0], "the held note let go");
        assert!(sent.contains(&[0xB3, 123, 0]) && sent.contains(&[0xB0, 64, 0]));
        // A forced channel rewrites every channel message.
        assert!(p.set_param("channel", 10.0));
        while h.pop(slot).is_some() {}
        p.note_on(0, 36, 90);
        assert_eq!(h.pop(slot).map(|x| x.1), Some([0x99, 36, 90]));
        // Offline (a bounce), nothing goes out.
        p.set_offline(true);
        p.note_on(0, 38, 90);
        assert_eq!(h.pop(slot), None);
        h.close(slot);
        assert!(!h.send(port_key("Test Synth A"), 0, [0x90, 1, 1]), "a closed port takes nothing");
    }
}
