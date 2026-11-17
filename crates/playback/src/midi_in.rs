//! Hardware MIDI input: USB MIDI keyboards and controllers, and an audio interface's 5-pin MIDI
//! In (both are MIDI input ports to the OS).
//!
//! - [`MidiHub`]: made once by the app and lent to the player ([`crate::Player::set_midi`]). One
//!   lock-free single-producer single-consumer [`MidiRing`] per port slot (atomics, preallocated,
//!   no `unsafe`): the port's driver callback pushes each message with its arrival time
//!   ([`PortSink::send`]); the audio thread drains every ring at the start of each mix block into
//!   the mix's [`LiveMidi`] ([`MidiHub::drain_into`]), each event at its sample offset within the
//!   block ([`block_offset`]: one block behind its arrival, so the timing within a block is kept;
//!   a late message lands at offset 0). Full rings drop and count. A second ring carries what
//!   the audio thread heard while recording back to the UI thread ([`MidiHub::take_recorded`]).
//! - [`MidiInputs`]: the UI thread's list of ports and their connections, through a
//!   [`MidiBackend`] (the system's, `midir` on WinMM/CoreMIDI/ALSA, native only; or a fake for
//!   tests). Ports are rescanned when asked ([`MidiInputs::rescan`]) and every 2 s
//!   ([`MidiInputs::tick`]); a port that disappears closes, a port that comes back reopens if
//!   enabled. Enabled ports are kept by name (app settings); a port never seen is enabled.

use soundcraft_mix::{LiveEvent, LiveMidi, MAX_MIDI_PORTS, port_id};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Messages one port's ring holds between two blocks.
pub const PORT_RING: usize = 1024;

/// Messages the record ring holds between two UI frames.
pub const RECORD_RING: usize = 16384;

/// The port slot of Musical Typing (the on-screen MIDI Keyboard and the computer keys): the
/// last one; hardware ports use the others.
pub const TYPING_SLOT: usize = MAX_MIDI_PORTS - 1;

/// The name Musical Typing has as a track input.
pub const TYPING_NAME: &str = "Musical Typing";

/// How often [`MidiInputs::tick`] rescans the ports.
pub const RESCAN_EVERY: Duration = Duration::from_secs(2);

/// Single-producer single-consumer FIFO of `(stamp, packed message)` pairs. The slots are
/// atomics, so it needs no lock and no `unsafe`; full, a push is dropped and counted.
pub struct MidiRing {
    slots: Box<[(AtomicU64, AtomicU64)]>,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
    dropped: AtomicU64,
}

impl MidiRing {
    pub fn new(capacity: usize) -> MidiRing {
        let n = capacity.clamp(2, 1 << 20).next_power_of_two();
        MidiRing {
            slots: (0..n).map(|_| (AtomicU64::new(0), AtomicU64::new(0))).collect(),
            mask: n - 1,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            dropped: AtomicU64::new(0),
        }
    }

    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Entries queued.
    pub fn len(&self) -> usize {
        self.head.load(Ordering::Acquire).wrapping_sub(self.tail.load(Ordering::Acquire)).min(self.slots.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Pushes failed for want of room, since it was made.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Producer side: queues one entry; false (and counted) when full.
    pub fn push(&self, stamp: u64, packed: u64) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        if head.wrapping_sub(self.tail.load(Ordering::Acquire)) >= self.slots.len() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        if let Some((a, b)) = self.slots.get(head & self.mask) {
            a.store(stamp, Ordering::Relaxed);
            b.store(packed, Ordering::Relaxed);
        }
        self.head.store(head.wrapping_add(1), Ordering::Release);
        true
    }

    /// Consumer side: takes the oldest entry.
    pub fn pop(&self) -> Option<(u64, u64)> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        let out = self.slots.get(tail & self.mask).map(|(a, b)| (a.load(Ordering::Relaxed), b.load(Ordering::Relaxed)));
        self.tail.store(tail.wrapping_add(1), Ordering::Release);
        out
    }
}

/// A channel or system-common message as 3 bytes (unused data bytes 0), `None` for what the mix
/// doesn't play: SysEx, timing clock, active sensing and the other real-time bytes, and anything
/// malformed (no status byte, or too short).
pub fn parse(bytes: &[u8]) -> Option<[u8; 3]> {
    let status = *bytes.first()?;
    if !(0x80..0xF0).contains(&status) {
        return None;
    }
    let need = if matches!(status & 0xF0, 0xC0 | 0xD0) { 2 } else { 3 };
    if bytes.len() < need {
        return None;
    }
    let d1 = bytes.get(1).copied().unwrap_or(0) & 0x7F;
    let d2 = if need == 3 { bytes.get(2).copied().unwrap_or(0) & 0x7F } else { 0 };
    Some([status, d1, d2])
}

fn pack(msg: [u8; 3]) -> u64 {
    u64::from(msg[0]) | (u64::from(msg[1]) << 8) | (u64::from(msg[2]) << 16)
}

fn unpack(v: u64) -> [u8; 3] {
    [(v & 0xFF) as u8, ((v >> 8) & 0xFF) as u8, ((v >> 16) & 0xFF) as u8]
}

/// Where a message stamped `stamp` (µs) lands in a block of `frames` rendered at `now` (µs) at
/// `rate` Hz: one block behind its arrival (a message that came as the previous block was
/// rendered is at offset 0, one that just came at the block's end), so the timing within a block
/// survives the block boundary. Older messages (late) are at 0; a stamp after `now` at the end.
pub fn block_offset(stamp: u64, now: u64, frames: usize, rate: f64) -> u32 {
    if frames == 0 {
        return 0;
    }
    let age_us = now.saturating_sub(stamp) as f64;
    let age = (age_us * rate / 1e6).max(0.0);
    let last = frames.saturating_sub(1) as f64;
    let at = (frames as f64 - age).clamp(0.0, last);
    at.floor() as u32
}

/// The loop pass of what Capture MIDI heard with the transport stopped: its position is then the
/// listening clock's (samples since the hub was made, at the session rate), not the timeline's.
pub const LISTENING: u16 = u16::MAX;

/// Setup › MIDI Input Filter's kinds as the hub's filter bits ([`MidiHub::set_filter`]), in the
/// order `setup.midi_input_filter` names them (SysEx and the real-time bytes are never played or
/// recorded, so they have no bit).
pub const FILTER_KINDS: [(&str, u32); 6] =
    [("notes", 1), ("pitch_bend", 2), ("mono_aftertouch", 4), ("poly_aftertouch", 8), ("program", 16), ("cc", 32)];

/// The filter bit of `msg`'s kind (0: none).
fn filter_bit(msg: [u8; 3]) -> u32 {
    match msg[0] & 0xF0 {
        0x80 | 0x90 => 1,
        0xE0 => 2,
        0xD0 => 4,
        0xA0 => 8,
        0xC0 => 16,
        0xB0 => 32,
        _ => 0,
    }
}

/// A recorded event: its timeline sample, port slot, message and loop pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordedMidi {
    pub at: i64,
    pub port: u8,
    pub msg: [u8; 3],
    /// Loop recording: the pass it was played in (0 for the first, and without a loop).
    pub pass: u16,
}

/// The rings, port identities and activity of the open MIDI input ports (see the module docs).
pub struct MidiHub {
    epoch: Instant,
    rings: [MidiRing; MAX_MIDI_PORTS],
    /// Each slot's port identity (`soundcraft_mix::port_id`), 0 when closed.
    ids: [AtomicU64; MAX_MIDI_PORTS],
    /// When each slot last received a message (µs since `epoch`, 0: never).
    activity: [AtomicU64; MAX_MIDI_PORTS],
    thru: AtomicBool,
    /// Setup › MIDI Input Filter ([`FILTER_KINDS`] bits).
    filter: AtomicU32,
    /// Slots whose port was plugged back in (bit per slot), for the next block's
    /// [`LiveMidi::reset`].
    reset: AtomicU64,
    /// Each slot's timing offset: its messages arrive this many µs after they were played.
    offsets: [AtomicU32; MAX_MIDI_PORTS],
    record: MidiRing,
    /// What the mix heard while playing and not recording, for Capture MIDI (`capture_block`).
    capture: MidiRing,
    /// The notes held now on any port and channel (bit per pitch), for naming a chord as it is
    /// played (the Chords ruler's picker).
    held: [AtomicU64; 2],
    /// Each slot's pedals down now: bit per channel, the sustain pedal (CC 64) in the low 16 bits,
    /// the sostenuto pedal (CC 66) in the high 16, so a take started with one down has it down.
    pedals: [AtomicU32; MAX_MIDI_PORTS],
    /// MIDI learn: the messages mappings claim, bit per (slot, CC or note, channel, number)
    /// ([`claim_bit`]). A claimed message goes to `control`, not to its port's ring: it is never
    /// heard, recorded or played through.
    claims: Box<[AtomicU64]>,
    /// While a control is being learnt, every CC and note is also handed over (flagged).
    learning: AtomicBool,
    /// Claimed and learnt messages, and MIDI Machine Control, for the UI thread (several ports'
    /// driver threads push: a multi-producer ring).
    control: soundcraft_mix::external_midi::OutRing,
    /// MIDI clock and timecode in: the sync messages the ports get, for the UI thread
    /// ([`MidiHub::take_sync`]), while [`MidiHub::set_sync_in`] is on.
    sync: soundcraft_mix::external_midi::OutRing,
    sync_on: AtomicBool,
}

/// A sync message handed to the UI thread by [`MidiHub::take_sync`]: the port slot, when it came
/// (µs on the hub's clock) and its bytes (`bytes[..len]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncMsg {
    pub slot: u8,
    pub at: u64,
    pub bytes: [u8; 10],
    pub len: usize,
}

impl SyncMsg {
    pub fn bytes(&self) -> &[u8] {
        self.bytes.get(..self.len).unwrap_or(&[])
    }
}

/// One message handed to the UI thread by [`MidiHub::take_control`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlMsg {
    pub slot: u8,
    pub msg: [u8; 3],
    /// Handed over because a control is being learnt.
    pub learn: bool,
    /// A MIDI Machine Control message (`F0 7F <device> 06 <command> F7`).
    pub mmc: Option<[u8; 6]>,
}

/// Bits in the claim table per port slot: CC and note, 16 channels, 128 numbers.
const CLAIM_BITS: usize = 2 * 16 * 128;

/// The claim table's bit for a message (`kind` 0 CC, 1 note).
fn claim_bit(slot: usize, kind: u8, channel: u8, number: u8) -> usize {
    slot * CLAIM_BITS + usize::from(kind & 1) * 2048 + usize::from(channel & 0x0F) * 128 + usize::from(number & 0x7F)
}

/// The claim kind of a message: 0 a CC, 1 a note on or off, `None` anything else.
fn claim_kind(msg: [u8; 3]) -> Option<u8> {
    match msg[0] & 0xF0 {
        0xB0 => Some(0),
        0x80 | 0x90 => Some(1),
        _ => None,
    }
}

impl MidiHub {
    pub fn new() -> Arc<MidiHub> {
        Arc::new(MidiHub {
            epoch: Instant::now(),
            rings: std::array::from_fn(|_| MidiRing::new(PORT_RING)),
            ids: std::array::from_fn(|_| AtomicU64::new(0)),
            activity: std::array::from_fn(|_| AtomicU64::new(0)),
            thru: AtomicBool::new(true),
            filter: AtomicU32::new(0),
            reset: AtomicU64::new(0),
            offsets: std::array::from_fn(|_| AtomicU32::new(0)),
            record: MidiRing::new(RECORD_RING),
            capture: MidiRing::new(RECORD_RING),
            held: [AtomicU64::new(0), AtomicU64::new(0)],
            pedals: std::array::from_fn(|_| AtomicU32::new(0)),
            claims: (0..MAX_MIDI_PORTS * CLAIM_BITS / 64).map(|_| AtomicU64::new(0)).collect(),
            learning: AtomicBool::new(false),
            control: soundcraft_mix::external_midi::OutRing::new(1024),
            sync: soundcraft_mix::external_midi::OutRing::new(1024),
            sync_on: AtomicBool::new(false),
        })
    }

    /// MIDI learn: the messages the mappings claim now, as (slot, kind 0 CC / 1 note, channel,
    /// number); everything else is unclaimed.
    pub fn set_claims(&self, claims: &[(usize, u8, u8, u8)]) {
        let mut words = vec![0u64; self.claims.len()];
        for &(slot, kind, ch, num) in claims {
            if slot < MAX_MIDI_PORTS {
                let b = claim_bit(slot, kind, ch, num);
                if let Some(w) = words.get_mut(b / 64) {
                    *w |= 1 << (b % 64);
                }
            }
        }
        for (a, w) in self.claims.iter().zip(words) {
            a.store(w, Ordering::Relaxed);
        }
    }

    /// While on, every CC and note is handed over too (MIDI learn is waiting for one).
    pub fn set_learning(&self, on: bool) {
        self.learning.store(on, Ordering::Relaxed);
    }

    fn claimed(&self, slot: usize, msg: [u8; 3]) -> bool {
        let Some(kind) = claim_kind(msg) else { return false };
        let b = claim_bit(slot, kind, msg[0] & 0x0F, msg[1]);
        self.claims.get(b / 64).is_some_and(|w| w.load(Ordering::Relaxed) & (1 << (b % 64)) != 0)
    }

    /// UI thread: what was claimed, learnt or sent as MMC since the last call.
    pub fn take_control(&self, out: &mut Vec<ControlMsg>) {
        while let Some((a, b)) = self.control.pop() {
            let slot = (a & 0xFF) as u8;
            if a & (1 << 9) != 0 {
                let bytes = b.to_le_bytes();
                out.push(ControlMsg { slot, msg: [0; 3], learn: false, mmc: Some([0xF0, 0x7F, bytes[0], 0x06, bytes[1], 0xF7]) });
            } else {
                out.push(ControlMsg { slot, msg: unpack(b), learn: a & (1 << 8) != 0, mmc: None });
            }
        }
    }

    /// Microseconds since the hub was made (never 0, which means "never").
    pub fn now_us(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_micros()).unwrap_or(u64::MAX).max(1)
    }

    /// MIDI Thru: record-armed instrument and MIDI tracks play their live input.
    /// MIDI clock and timecode in on or off: on, the ports' sync messages are kept for
    /// [`MidiHub::take_sync`].
    pub fn set_sync_in(&self, on: bool) {
        self.sync_on.store(on, Ordering::Relaxed);
    }

    /// UI thread: the sync messages since the last call, oldest first.
    pub fn take_sync(&self, out: &mut Vec<SyncMsg>) {
        while let Some((a, b)) = self.sync.pop() {
            let p = b.to_le_bytes();
            let (slot, at) = ((a & 0xFF) as u8, a >> 8);
            let msg = if p[0] == 0xF0 {
                // A full frame, packed as F0 hr mn sc fr.
                SyncMsg { slot, at, bytes: [0xF0, 0x7F, 0x7F, 0x01, 0x01, p[1], p[2], p[3], p[4], 0xF7], len: 10 }
            } else {
                let len = match p[0] {
                    0xF2 => 3,
                    0xF1 => 2,
                    _ => 1,
                };
                SyncMsg { slot, at, bytes: [p[0], p[1], p[2], 0, 0, 0, 0, 0, 0, 0], len }
            };
            out.push(msg);
        }
    }

    pub fn set_thru(&self, on: bool) {
        self.thru.store(on, Ordering::Relaxed);
    }

    /// The input filter ([`FILTER_KINDS`] bits): messages of those kinds from every port are
    /// left out of what the tracks hear, record and capture (a mapped control still moves its
    /// parameter).
    pub fn set_filter(&self, bits: u32) {
        self.filter.store(bits, Ordering::Relaxed);
    }

    pub fn thru(&self) -> bool {
        self.thru.load(Ordering::Relaxed)
    }

    /// Slot `slot`'s port went away and came back (or was reopened): the notes and pedals it
    /// held are let go on the next block.
    fn reset_slot(&self, slot: usize) {
        if slot < MAX_MIDI_PORTS {
            self.reset.fetch_or(1u64 << slot, Ordering::Relaxed);
        }
        if let Some(p) = self.pedals.get(slot) {
            p.store(0, Ordering::Relaxed);
        }
    }

    /// Slot `slot`'s messages arrive `us` µs after they were played (a device's own delay):
    /// they are placed that much earlier.
    pub fn set_offset(&self, slot: usize, us: u32) {
        if let Some(o) = self.offsets.get(slot) {
            o.store(us, Ordering::Relaxed);
        }
    }

    /// A port slot holds the port named `name` (`None`: closed).
    fn set_slot(&self, slot: usize, name: Option<&str>) {
        if let Some(id) = self.ids.get(slot) {
            id.store(name.map_or(0, port_id), Ordering::Release);
        }
    }

    /// When slot `slot` last received a message, µs since the hub was made (0: never).
    pub fn last_activity(&self, slot: usize) -> u64 {
        self.activity.get(slot).map_or(0, |a| a.load(Ordering::Relaxed))
    }

    /// The pitches held down now on any open port (Musical Typing included), lowest first.
    pub fn held_notes(&self) -> Vec<u8> {
        let words = [self.held[0].load(Ordering::Relaxed), self.held[1].load(Ordering::Relaxed)];
        (0..128u8).filter(|p| words[usize::from(p / 64)] & (1u64 << (p % 64)) != 0).collect()
    }

    /// Forgets the notes held (All MIDI Notes Off: their note-offs may never come).
    pub fn clear_held(&self) {
        self.held.iter().for_each(|w| w.store(0, Ordering::Relaxed));
    }

    /// Slot `slot`'s pedals as the message from it leaves them.
    fn pedal(&self, slot: usize, msg: [u8; 3]) {
        if msg[0] & 0xF0 != 0xB0 || !matches!(msg[1], 64 | 66) {
            return;
        }
        let Some(p) = self.pedals.get(slot) else { return };
        let bit = 1u32 << (u32::from(msg[0] & 0x0F) + if msg[1] == 66 { 16 } else { 0 });
        if msg[2] >= 64 {
            p.fetch_or(bit, Ordering::Relaxed);
        } else {
            p.fetch_and(!bit, Ordering::Relaxed);
        }
    }

    /// Audio thread, on a take's first block: the pedals down now on the open ports, pressed at
    /// timeline `at` in pass `pass` (a pedal pressed before Record is down in the take). Never
    /// allocates.
    pub fn record_pedals(&self, at: i64, pass: u16) {
        for (slot, p) in self.pedals.iter().enumerate() {
            let bits = p.load(Ordering::Relaxed);
            if bits == 0 || self.ids.get(slot).is_none_or(|i| i.load(Ordering::Acquire) == 0) {
                continue;
            }
            for k in 0..32u32 {
                if bits & (1 << k) != 0 {
                    let msg = [0xB0 | (k % 16) as u8, if k >= 16 { 66 } else { 64 }, 127];
                    let packed = pack(msg) | ((slot as u64 & 0xFF) << 24) | (u64::from(pass) << 32);
                    self.record.push(at as u64, packed);
                }
            }
        }
    }

    fn note_held(&self, msg: [u8; 3]) {
        let (kind, pitch) = (msg[0] & 0xF0, msg[1] & 0x7F);
        let Some(word) = self.held.get(usize::from(pitch / 64)) else { return };
        let bit = 1u64 << (pitch % 64);
        match kind {
            0x90 if msg[2] > 0 => {
                word.fetch_or(bit, Ordering::Relaxed);
            }
            0x80 | 0x90 => {
                word.fetch_and(!bit, Ordering::Relaxed);
            }
            // All Notes Off / All Sound Off.
            0xB0 if msg[1] == 123 || msg[1] == 120 => self.held.iter().for_each(|w| w.store(0, Ordering::Relaxed)),
            _ => {}
        }
    }

    /// Messages dropped because a port's ring was full, all ports together.
    pub fn dropped(&self) -> u64 {
        self.rings.iter().map(MidiRing::dropped).sum()
    }

    /// A sender for slot `slot` (one per port connection: each ring has one producer).
    pub fn sink(self: &Arc<Self>, slot: usize) -> PortSink {
        PortSink { hub: Arc::clone(self), slot }
    }

    /// Musical Typing's sender (its producer is the UI thread), its slot opened as the port
    /// [`TYPING_NAME`].
    pub fn typing(self: &Arc<Self>) -> PortSink {
        self.set_slot(TYPING_SLOT, Some(TYPING_NAME));
        self.sink(TYPING_SLOT)
    }

    /// Whether any port slot is open.
    pub fn any_open(&self) -> bool {
        self.ids.iter().any(|i| i.load(Ordering::Relaxed) != 0)
    }

    /// Audio thread, once per mix block of `frames` at `rate` Hz, at `now` ([`MidiHub::now_us`]):
    /// empties every port's ring into `live` (cleared first) at block offsets, sorted, with the
    /// slots' port identities and Thru. Never allocates or blocks.
    pub fn drain_into(&self, live: &mut LiveMidi, now: u64, frames: usize, rate: f64) {
        live.clear();
        live.thru = self.thru();
        live.reset = self.reset.swap(0, Ordering::Relaxed);
        for (slot, ring) in self.rings.iter().enumerate() {
            let id = self.ids.get(slot).map_or(0, |i| i.load(Ordering::Acquire));
            if let Some(p) = live.ports.get_mut(slot) {
                *p = id;
            }
            while let Some((stamp, packed)) = ring.pop() {
                if id == 0 {
                    continue;
                }
                let offset = block_offset(stamp, now, frames, rate);
                live.push(LiveEvent { offset, port: slot as u8, msg: unpack(packed), stamp });
            }
        }
        live.sort();
    }

    /// Audio thread: empties the rings unheard (nothing renders this block).
    pub fn discard(&self) {
        for ring in &self.rings {
            while ring.pop().is_some() {}
        }
    }

    /// Audio thread, while recording: the block's events for the UI thread, with the loop pass,
    /// each at its raw age: `base` is the timeline position heard at `now` (µs: the callback's
    /// start), and an event stamped earlier is that much earlier at `rate` Hz (one stamped later,
    /// later). Not from the block offsets, which are clamped to the block: a block cut short by a
    /// loop's end or a callback split in blocks would move older events to its start. Dropped
    /// (counted) when the UI falls behind. Never allocates.
    /// No event lands before `floor` (the take's first frame on the first block after Play: a
    /// note played between the Record press and the transport rolling belongs at its start).
    pub fn record_block(&self, live: &LiveMidi, base: i64, now: u64, rate: f64, pass: u16, floor: i64) {
        // The age is real time, turned into timeline frames at full speed: right because a take
        // only ever records at full speed (`Player::can_record` and `Player::set_speed` refuse
        // varispeed while recording). Were that to change, the age would scale by the speed.
        heard_into(&self.record, live, base, now, rate, pass, floor);
    }

    /// Audio thread, while playing and not recording: the block's events for Capture MIDI, placed
    /// as [`MidiHub::record_block`] places them (no floor). The ring keeps the newest: when the
    /// UI falls behind, what does not fit is dropped. Never allocates.
    pub fn capture_block(&self, live: &LiveMidi, base: i64, now: u64, rate: f64, pass: u16) {
        heard_into(&self.capture, live, base, now, rate, pass, i64::MIN);
    }

    /// Audio thread, with the transport stopped: the block's events for Capture MIDI on the
    /// listening clock (each at its stamp, in samples at `rate` since the hub was made), with the
    /// pass [`LISTENING`]. Never allocates.
    pub fn listen_block(&self, live: &LiveMidi, rate: f64) {
        for e in live.events() {
            let at = (e.stamp as f64 * rate / 1e6).round() as i64;
            let packed = pack(e.msg) | (u64::from(e.port) << 24) | (u64::from(LISTENING) << 32);
            self.capture.push(at as u64, packed);
        }
    }

    /// UI thread: what was captured since the last call.
    pub fn take_captured(&self, out: &mut Vec<RecordedMidi>) {
        drain_heard(&self.capture, out);
    }

    /// UI thread: what was recorded since the last call.
    pub fn take_recorded(&self, out: &mut Vec<RecordedMidi>) {
        drain_heard(&self.record, out);
    }

    /// Recorded events lost because the UI thread fell behind.
    pub fn record_dropped(&self) -> u64 {
        self.record.dropped()
    }
}

/// `live`'s events into `ring`, each at its timeline position from its raw age (see
/// [`MidiHub::record_block`]), not before `floor`, with its port slot and loop pass.
fn heard_into(ring: &MidiRing, live: &LiveMidi, base: i64, now: u64, rate: f64, pass: u16, floor: i64) {
    for e in live.events() {
        let age = ((now as f64 - e.stamp as f64) * rate / 1e6).round() as i64;
        let at = base.saturating_sub(age).max(floor);
        let packed = pack(e.msg) | (u64::from(e.port) << 24) | (u64::from(pass) << 32);
        ring.push(at as u64, packed);
    }
}

/// Everything in a ring [`heard_into`] filled, as recorded events.
fn drain_heard(ring: &MidiRing, out: &mut Vec<RecordedMidi>) {
    while let Some((at, packed)) = ring.pop() {
        out.push(RecordedMidi { at: at as i64, port: ((packed >> 24) & 0xFF) as u8, msg: unpack(packed), pass: ((packed >> 32) & 0xFFFF) as u16 });
    }
}

/// The sending end of one port slot's ring, handed to the port's driver callback.
pub struct PortSink {
    hub: Arc<MidiHub>,
    slot: usize,
}

impl PortSink {
    /// The hub's clock now (µs since it was made).
    pub fn now_us(&self) -> u64 {
        self.hub.now_us()
    }

    /// One raw message from the port, stamped now. What the mix doesn't play is ignored.
    pub fn send(&self, bytes: &[u8]) {
        self.send_at(self.hub.now_us(), bytes);
    }

    /// [`PortSink::send`] stamped `stamp` (µs since the hub was made), less the port's timing
    /// offset ([`MidiHub::set_offset`]).
    pub fn send_at(&self, stamp: u64, bytes: &[u8]) {
        let late = self.hub.offsets.get(self.slot).map_or(0, |o| o.load(Ordering::Relaxed));
        let stamp = stamp.saturating_sub(u64::from(late)).max(1);
        // MIDI clock and timecode in, for the UI thread (`sync_in`).
        if crate::sync_in::is_sync(bytes) {
            if self.hub.sync_on.load(Ordering::Relaxed) {
                let p = match bytes {
                    [0xF0, _, _, _, _, hr, mn, sc, fr, ..] => [0xF0, *hr, *mn, *sc, *fr, 0, 0, 0],
                    _ => [bytes.first().copied().unwrap_or(0), bytes.get(1).copied().unwrap_or(0), bytes.get(2).copied().unwrap_or(0), 0, 0, 0, 0, 0],
                };
                self.hub.sync.push((stamp << 8) | (self.slot as u64 & 0xFF), u64::from_le_bytes(p));
            }
            return;
        }
        // MIDI Machine Control: the transport, for the UI thread.
        if let [0xF0, 0x7F, device, 0x06, command, ..] = bytes {
            let (a, b) = ((self.slot as u64 & 0xFF) | (1 << 9), u64::from_le_bytes([*device, *command, 0, 0, 0, 0, 0, 0]));
            self.hub.control.push(a, b);
            return;
        }
        let Some(msg) = parse(bytes) else { return };
        let filtered = self.hub.filter.load(Ordering::Relaxed) & filter_bit(msg) != 0;
        let learning = self.hub.learning.load(Ordering::Relaxed) && claim_kind(msg).is_some();
        let note_off = msg[0] & 0xF0 == 0x80 || (msg[0] & 0xF0 == 0x90 && msg[2] == 0);
        if learning && !note_off {
            // (Not a note-off: the key let go after learning would be learnt again.)
            self.hub.control.push((self.slot as u64 & 0xFF) | (1 << 8), pack(msg));
        }
        if self.hub.claimed(self.slot, msg) {
            // A mapped control: it moves its parameter, it never plays or records.
            if let Some(a) = self.hub.activity.get(self.slot) {
                a.store(stamp.max(1), Ordering::Relaxed);
            }
            if !learning {
                self.hub.control.push(self.slot as u64 & 0xFF, pack(msg));
            }
            return;
        }
        if let Some(a) = self.hub.activity.get(self.slot) {
            a.store(stamp.max(1), Ordering::Relaxed);
        }
        if filtered {
            // Setup › MIDI Input Filter leaves it out.
            return;
        }
        self.hub.note_held(msg);
        self.hub.pedal(self.slot, msg);
        if let Some(r) = self.hub.rings.get(self.slot) {
            r.push(stamp, pack(msg));
        }
    }
}

/// An open port: dropping it closes the port.
pub trait MidiConnection: Send {}

/// Where ports come from: the system's (`midir`), or a fake for tests.
pub trait MidiBackend: Send {
    /// The input ports there are now, by name.
    fn ports(&mut self) -> Vec<String>;
    /// Opens the port named `name`, its messages going to `sink`.
    fn open(&mut self, name: &str, sink: PortSink) -> Result<Box<dyn MidiConnection>, String>;
    /// Each port's system identity by name, where the system gives one that changes when a
    /// device is plugged in again (ALSA's client and port; CoreMIDI and WinMM keep theirs, so
    /// there a quick unplug and replug cannot be told). Empty: no identities.
    fn ids(&mut self) -> Vec<(String, String)> {
        Vec::new()
    }
}

/// Port names made unique: a second port with a name already listed (two identical
/// keyboards) is `<name> #2`, a third `<name> #3`, in the system's order.
pub fn unique_names(raw: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(raw.len());
    for n in raw {
        let mut name = n.clone();
        let mut k = 1;
        while out.contains(&name) {
            k += 1;
            name = format!("{n} #{k}");
        }
        out.push(name);
    }
    out
}

/// One port as the MIDI Input Devices window lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInfo {
    pub name: String,
    pub enabled: bool,
    /// Present on the system now.
    pub present: bool,
    /// Open (present, enabled and opened).
    pub open: bool,
    /// Why it could not open.
    pub error: Option<String>,
    /// Its ring slot in the hub.
    pub slot: usize,
}

struct Port {
    info: PortInfo,
    conn: Option<Box<dyn MidiConnection>>,
    /// The system's identity of the port when it was opened (see [`MidiBackend::ids`]).
    id: Option<String>,
}

/// The UI thread's MIDI input ports (see the module docs).
pub struct MidiInputs {
    backend: Box<dyn MidiBackend>,
    hub: Arc<MidiHub>,
    ports: Vec<Port>,
    /// Enabled (true) or disabled by port name, as saved; a port not named is enabled.
    choices: BTreeMap<String, bool>,
    /// Timing offsets by port name, in ms (`set_offset`).
    offsets: BTreeMap<String, f32>,
    /// Ports control surfaces use: closed here (a surface's messages are not notes).
    reserved: Vec<String>,
    last_scan: Option<Instant>,
}

impl MidiInputs {
    pub fn new(backend: Box<dyn MidiBackend>, hub: Arc<MidiHub>, choices: BTreeMap<String, bool>) -> MidiInputs {
        MidiInputs { backend, hub, ports: Vec::new(), choices, reserved: Vec::new(), offsets: BTreeMap::new(), last_scan: None }
    }

    /// The system's ports (`None` where there is no MIDI backend: the web build).
    pub fn system(hub: Arc<MidiHub>, choices: BTreeMap<String, bool>) -> Option<MidiInputs> {
        system_backend().map(|b| MidiInputs::new(b, hub, choices))
    }

    pub fn hub(&self) -> &Arc<MidiHub> {
        &self.hub
    }

    /// Every port seen since the app started (present or not), in the order first seen.
    pub fn ports(&self) -> Vec<PortInfo> {
        self.ports.iter().map(|p| p.info.clone()).collect()
    }

    /// The names of the ports present now.
    pub fn present(&self) -> Vec<String> {
        self.ports.iter().filter(|p| p.info.present).map(|p| p.info.name.clone()).collect()
    }

    /// The enabled choices to save (by port name).
    pub fn choices(&self) -> &BTreeMap<String, bool> {
        &self.choices
    }

    /// Whether `name` is enabled (a port never chosen is).
    pub fn enabled(&self, name: &str) -> bool {
        self.choices.get(name).copied().unwrap_or(true)
    }

    /// Enables or disables the port named `name` (saved by name), opening or closing it now.
    pub fn set_enabled(&mut self, name: &str, on: bool) {
        self.choices.insert(name.to_string(), on);
        self.apply();
    }

    /// The ports control surfaces use: closed here, so what they send never reaches a track
    /// (takes, Thru, All Inputs).
    pub fn set_reserved(&mut self, names: Vec<String>) {
        if names != self.reserved {
            self.reserved = names;
            self.apply();
        }
    }

    /// Whether a control surface uses `name`.
    pub fn reserved(&self, name: &str) -> bool {
        self.reserved.iter().any(|r| r == name)
    }

    /// Each port's timing offset by name, in ms: its messages arrive that late (0..100).
    pub fn set_offsets(&mut self, offsets: BTreeMap<String, f32>) {
        if offsets != self.offsets {
            self.offsets = offsets;
            for p in &self.ports {
                self.hub.set_offset(p.info.slot, offset_us(&self.offsets, &p.info.name));
            }
        }
    }

    /// When the port named `name` last received a message, if it is open.
    pub fn since_activity(&self, name: &str) -> Option<Duration> {
        let p = self.ports.iter().find(|p| p.info.name == name && p.info.open)?;
        let at = self.hub.last_activity(p.info.slot);
        (at != 0).then(|| Duration::from_micros(self.hub.now_us().saturating_sub(at)))
    }

    /// Rescans when the last scan is [`RESCAN_EVERY`] old (or there was none).
    pub fn tick(&mut self, now: Instant) {
        if self.last_scan.is_none_or(|t| now.saturating_duration_since(t) >= RESCAN_EVERY) {
            self.rescan_at(now);
        }
    }

    /// Rescans the ports now: new ones are listed, gone ones close, enabled ones open.
    pub fn rescan(&mut self) {
        self.rescan_at(Instant::now());
    }

    fn rescan_at(&mut self, now: Instant) {
        self.last_scan = Some(now);
        let names = self.backend.ports();
        let ids = self.backend.ids();
        for p in &mut self.ports {
            p.info.present = names.contains(&p.info.name);
            // Plugged in again since the last scan (its identity changed): what it held is
            // let go, and it is opened afresh.
            let id = ids.iter().find(|(n, _)| *n == p.info.name).map(|(_, i)| i.clone());
            if p.conn.is_some() && p.info.present && id.is_some() && p.id.is_some() && id != p.id {
                p.conn = None;
                self.hub.set_slot(p.info.slot, None);
                self.hub.reset_slot(p.info.slot);
            }
        }
        for n in names {
            if self.ports.iter().any(|p| p.info.name == n) {
                continue;
            }
            // A slot no port ever had, else one whose port is gone (the oldest first).
            let used: Vec<usize> = self.ports.iter().map(|p| p.info.slot).collect();
            let slot = match (0..TYPING_SLOT).find(|s| !used.contains(s)) {
                Some(s) => s,
                None => match self.ports.iter().position(|p| !p.info.present && p.conn.is_none()) {
                    Some(i) => self.ports.remove(i).info.slot,
                    None => continue,
                },
            };
            self.ports.push(Port { info: PortInfo { name: n, enabled: true, present: true, open: false, error: None, slot }, conn: None, id: None });
        }
        self.apply();
        for p in self.ports.iter_mut().filter(|p| p.conn.is_some()) {
            if p.id.is_none() {
                p.id = ids.iter().find(|(n, _)| *n == p.info.name).map(|(_, i)| i.clone());
            }
        }
    }

    /// Opens what should be open and closes the rest.
    fn apply(&mut self) {
        for i in 0..self.ports.len() {
            let Some(p) = self.ports.get(i) else { continue };
            let enabled = self.enabled(&p.info.name);
            let want = enabled && p.info.present && !self.reserved.contains(&p.info.name);
            let (name, slot, has) = (p.info.name.clone(), p.info.slot, p.conn.is_some());
            let mut result = None;
            if want && !has {
                // The slot starts empty: whatever an old port left in it is not this port's.
                self.hub.set_slot(slot, Some(&name));
                self.hub.set_offset(slot, offset_us(&self.offsets, &name));
                result = Some(self.backend.open(&name, self.hub.sink(slot)));
            }
            let Some(p) = self.ports.get_mut(i) else { continue };
            p.info.enabled = enabled;
            match result {
                Some(Ok(c)) => {
                    p.conn = Some(c);
                    p.id = None;
                    p.info.error = None;
                }
                Some(Err(err)) => {
                    log::warn!("MIDI input {name}: {err}");
                    p.info.error = Some(err);
                    self.hub.set_slot(slot, None);
                }
                None if !want => {
                    // Closing drops the connection: the driver stops calling back.
                    if p.conn.take().is_some() {
                        self.hub.reset_slot(slot);
                    }
                    p.id = None;
                    self.hub.set_slot(slot, None);
                }
                None => {}
            }
            p.info.open = p.conn.is_some();
        }
    }
}

/// `name`'s timing offset in µs (0..100 ms).
fn offset_us(offsets: &BTreeMap<String, f32>, name: &str) -> u32 {
    offsets.get(name).filter(|v| v.is_finite()).map_or(0, |ms| (ms.clamp(0.0, 100.0) * 1000.0) as u32)
}

/// Turns a driver's timestamps (µs on its own clock) into the hub's: the smallest gap seen
/// between the hub's clock and the driver's is the delivery delay, so a message keeps the time
/// the driver gave it however late its callback ran. The gap may grow by 100 ppm of the time
/// since (the two clocks drift), and a stamp is never after now.
#[derive(Debug, Clone, Copy, Default)]
pub struct DriverClock {
    /// (hub − driver, at hub time).
    gap: Option<(i64, u64)>,
}

impl DriverClock {
    pub fn to_hub(&mut self, driver: u64, now: u64) -> u64 {
        // No stamp from the driver: now.
        if driver == 0 {
            return now;
        }
        let seen = now as i64 - driver as i64;
        let gap = match self.gap {
            Some((g, at)) => seen.min(g.saturating_add((now.saturating_sub(at) / 10_000) as i64)),
            None => seen,
        };
        let at = (driver as i64).saturating_add(gap).clamp(1, now.max(1) as i64) as u64;
        // More than a second behind: the driver's clock jumped; start over from this message.
        if now.saturating_sub(at) > 1_000_000 {
            self.gap = Some((seen, now));
            return now;
        }
        self.gap = Some((gap, now));
        at
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod system {
    use super::{MidiBackend, MidiConnection, PortSink};

    pub struct Midir;

    struct Conn(#[allow(dead_code)] midir::MidiInputConnection<()>);

    impl MidiConnection for Conn {}

    fn client() -> Result<midir::MidiInput, String> {
        let mut m = midir::MidiInput::new("FreeDAWm").map_err(|e| e.to_string())?;
        // SysEx comes through (MIDI Machine Control); the clock and active sensing do not.
        m.ignore(midir::Ignore::ActiveSense);
        Ok(m)
    }

    struct RawConn(#[allow(dead_code)] midir::MidiInputConnection<()>);

    impl MidiConnection for RawConn {}

    pub fn open_raw(name: &str, to: std::sync::mpsc::Sender<Vec<u8>>) -> Result<Box<dyn MidiConnection>, String> {
        let mut m = midir::MidiInput::new("FreeDAWm surface").map_err(|e| e.to_string())?;
        // SysEx too (handshakes), but not the clock and active sensing.
        m.ignore(midir::Ignore::ActiveSense);
        let port = m.ports().into_iter().find(|p| m.port_name(p).ok().as_deref() == Some(name)).ok_or_else(|| "not found".to_string())?;
        let conn = m
            .connect(
                &port,
                "FreeDAWm surface input",
                move |_stamp, bytes, _| {
                    let _ = to.send(bytes.to_vec());
                },
                (),
            )
            .map_err(|e| e.to_string())?;
        Ok(Box::new(RawConn(conn)))
    }

    /// The system's ports with their names made unique (two identical keyboards: `Keys` and
    /// `Keys #2`).
    fn listed(m: &midir::MidiInput) -> Vec<(String, midir::MidiInputPort)> {
        let ports = m.ports();
        let names = super::unique_names(ports.iter().map(|p| m.port_name(p).unwrap_or_default()).collect());
        names.into_iter().zip(ports).filter(|(n, _)| !n.is_empty()).collect()
    }

    impl MidiBackend for Midir {
        fn ports(&mut self) -> Vec<String> {
            let Ok(m) = client() else { return Vec::new() };
            listed(&m).into_iter().map(|(n, _)| n).collect()
        }

        fn ids(&mut self) -> Vec<(String, String)> {
            let Ok(m) = client() else { return Vec::new() };
            listed(&m).into_iter().map(|(n, p)| (n, p.id())).collect()
        }

        fn open(&mut self, name: &str, sink: PortSink) -> Result<Box<dyn MidiConnection>, String> {
            let m = client()?;
            let port = listed(&m).into_iter().find(|(n, _)| n == name).map(|(_, p)| p).ok_or_else(|| "not found".to_string())?;
            // The driver's own timestamp, where it gives one, on the hub's clock.
            let mut clock = super::DriverClock::default();
            let conn = m
                .connect(&port, "FreeDAWm input", move |stamp, bytes, _| sink.send_at(clock.to_hub(stamp, sink.now_us()), bytes), ())
                .map_err(|e| e.to_string())?;
            Ok(Box::new(Conn(conn)))
        }
    }
}

/// Opens the system's input port `name` for a control surface: every message, SysEx included,
/// goes to `to` as it comes (from the driver's thread). Dropping the connection closes the port.
pub fn open_raw_input(name: &str, to: std::sync::mpsc::Sender<Vec<u8>>) -> Result<Box<dyn MidiConnection>, String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        system::open_raw(name, to)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = to;
        Err(format!("{name}: no MIDI input here"))
    }
}

/// The system's input port names (empty on the web).
pub fn system_port_names() -> Vec<String> {
    system_backend().map(|mut b| b.ports()).unwrap_or_default()
}

/// The system's MIDI backend (`None` on the web).
pub fn system_backend() -> Option<Box<dyn MidiBackend>> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Some(Box::new(system::Midir))
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

/// A backend with ports a test adds and removes, and the sinks of the ports it opened (to
/// play into them as a keyboard would). Shared state, so a test keeps a handle while the
/// [`MidiInputs`] owns the backend.
#[derive(Clone, Default)]
pub struct FakeBackend {
    state: Arc<std::sync::Mutex<FakeState>>,
}

#[derive(Default)]
struct FakeState {
    ports: Vec<String>,
    /// How often each port was plugged in again (its identity).
    plugged: Vec<(String, u32)>,
    sinks: Vec<(String, Arc<PortSink>)>,
    refuse: Vec<String>,
}

struct FakeConn {
    state: Arc<std::sync::Mutex<FakeState>>,
    name: String,
}

impl MidiConnection for FakeConn {}

impl Drop for FakeConn {
    fn drop(&mut self) {
        if let Ok(mut s) = self.state.lock() {
            s.sinks.retain(|(n, _)| *n != self.name);
        }
    }
}

impl FakeBackend {
    pub fn with_ports(names: &[&str]) -> FakeBackend {
        let f = FakeBackend::default();
        if let Ok(mut s) = f.state.lock() {
            s.ports = names.iter().map(|n| n.to_string()).collect();
        }
        f
    }

    /// Plugs in (or keeps) the port named `name`.
    pub fn plug(&self, name: &str) {
        if let Ok(mut s) = self.state.lock()
            && !s.ports.iter().any(|p| p == name)
        {
            s.ports.push(name.to_string());
        }
    }

    /// Unplugs the port named `name`.
    pub fn unplug(&self, name: &str) {
        if let Ok(mut s) = self.state.lock() {
            s.ports.retain(|p| p != name);
        }
    }

    /// Unplugs `name` and plugs it in again between two scans: the port's system identity
    /// changes (as ALSA's does).
    pub fn replug(&self, name: &str) {
        if let Ok(mut s) = self.state.lock() {
            match s.plugged.iter_mut().find(|(n, _)| n == name) {
                Some((_, k)) => *k += 1,
                None => s.plugged.push((name.to_string(), 1)),
            }
        }
    }

    /// Opening `name` fails from now on.
    pub fn refuse(&self, name: &str) {
        if let Ok(mut s) = self.state.lock() {
            s.refuse.push(name.to_string());
        }
    }

    /// Whether `name` is open.
    pub fn is_open(&self, name: &str) -> bool {
        self.state.lock().is_ok_and(|s| s.sinks.iter().any(|(n, _)| n == name))
    }

    /// Plays `bytes` into the open port `name` (false when it is not open).
    pub fn send(&self, name: &str, bytes: &[u8]) -> bool {
        let sink = self.state.lock().ok().and_then(|s| s.sinks.iter().find(|(n, _)| n == name).map(|(_, k)| Arc::clone(k)));
        sink.map(|k| k.send(bytes)).is_some()
    }

    /// The boxed backend for [`MidiInputs::new`].
    pub fn boxed(&self) -> Box<dyn MidiBackend> {
        Box::new(self.clone())
    }
}

impl MidiBackend for FakeBackend {
    fn ports(&mut self) -> Vec<String> {
        self.state.lock().map(|s| unique_names(s.ports.clone())).unwrap_or_default()
    }

    fn ids(&mut self) -> Vec<(String, String)> {
        let Ok(s) = self.state.lock() else { return Vec::new() };
        unique_names(s.ports.clone())
            .into_iter()
            .map(|n| {
                let k = s.plugged.iter().find(|(p, _)| *p == n).map_or(0, |(_, k)| *k);
                (n.clone(), format!("{n}/{k}"))
            })
            .collect()
    }

    fn open(&mut self, name: &str, sink: PortSink) -> Result<Box<dyn MidiConnection>, String> {
        let mut s = self.state.lock().map_err(|_| "poisoned".to_string())?;
        if s.refuse.iter().any(|r| r == name) {
            return Err("in use by another program".into());
        }
        if !unique_names(s.ports.clone()).iter().any(|p| p == name) {
            return Err("not found".into());
        }
        s.sinks.push((name.to_string(), Arc::new(sink)));
        Ok(Box::new(FakeConn { state: Arc::clone(&self.state), name: name.to_string() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_messages_reach_the_ui_only_while_sync_in_is_on() {
        let hub = MidiHub::new();
        let sink = PortSink { hub: Arc::clone(&hub), slot: 2 };
        sink.send_at(100, &[0xFA]);
        let mut got = Vec::new();
        hub.take_sync(&mut got);
        assert!(got.is_empty(), "off: dropped");
        hub.set_sync_in(true);
        sink.send_at(200, &[0xF8]);
        sink.send_at(300, &[0xF2, 4, 1]);
        sink.send_at(400, &[0xF1, 0x23]);
        sink.send_at(500, &[0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x21, 2, 3, 4, 0xF7]);
        hub.take_sync(&mut got);
        let seen: Vec<(u8, u64, Vec<u8>)> = got.iter().map(|m| (m.slot, m.at, m.bytes().to_vec())).collect();
        assert_eq!(
            seen,
            vec![
                (2, 200, vec![0xF8]),
                (2, 300, vec![0xF2, 4, 1]),
                (2, 400, vec![0xF1, 0x23]),
                (2, 500, vec![0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x21, 2, 3, 4, 0xF7])
            ]
        );
        // Never played or recorded.
        assert!(hub.rings[2].pop().is_none());
    }

    #[test]
    fn a_claimed_control_is_handed_over_instead_of_played_and_mmc_too() {
        let hub = MidiHub::new();
        let k = hub.sink(3);
        hub.set_slot(3, Some("Knobs"));
        hub.set_claims(&[(3, 0, 0, 7)]);
        k.send(&[0xB0, 7, 99]);
        k.send(&[0xB1, 7, 98]);
        k.send(&[0xF0, 0x7F, 0x10, 0x06, 0x02, 0xF7]);
        let mut live = LiveMidi::new();
        hub.drain_into(&mut live, hub.now_us(), 64, 48_000.0);
        assert_eq!(live.events().iter().map(|e| e.msg).collect::<Vec<_>>(), vec![[0xB1, 7, 98]], "only the unclaimed channel plays");
        let mut got = Vec::new();
        hub.take_control(&mut got);
        assert_eq!(got[0], ControlMsg { slot: 3, msg: [0xB0, 7, 99], learn: false, mmc: None });
        assert_eq!(got[1].mmc, Some([0xF0, 0x7F, 0x10, 0x06, 0x02, 0xF7]));
        // Learning: every CC and note-on is handed over, flagged, and still plays.
        hub.set_learning(true);
        k.send(&[0x90, 60, 100]);
        k.send(&[0x80, 60, 0]);
        got.clear();
        hub.take_control(&mut got);
        assert_eq!(got, vec![ControlMsg { slot: 3, msg: [0x90, 60, 100], learn: true, mmc: None }]);
        hub.drain_into(&mut live, hub.now_us(), 64, 48_000.0);
        assert_eq!(live.events().len(), 2);
    }

    #[test]
    fn a_port_a_control_surface_uses_is_closed_to_notes() {
        let fake = FakeBackend::with_ports(&["MCU", "Keys"]);
        let mut m = MidiInputs::new(fake.boxed(), MidiHub::new(), BTreeMap::new());
        m.rescan();
        m.set_reserved(vec!["MCU".into()]);
        assert!(!fake.is_open("MCU") && fake.is_open("Keys"));
        assert!(m.reserved("MCU"));
        m.set_reserved(Vec::new());
        assert!(fake.is_open("MCU"));
    }

    #[test]
    fn ring_keeps_order_and_counts_overflow() {
        let r = MidiRing::new(4);
        assert_eq!(r.capacity(), 4);
        for i in 0..6u64 {
            r.push(i, i * 10);
        }
        assert_eq!((r.len(), r.dropped()), (4, 2));
        let got: Vec<(u64, u64)> = std::iter::from_fn(|| r.pop()).collect();
        assert_eq!(got, vec![(0, 0), (1, 10), (2, 20), (3, 30)]);
        // Room again after draining; wrapping around the slots keeps the order.
        for i in 0..10u64 {
            assert!(r.push(i, i));
            assert_eq!(r.pop(), Some((i, i)));
        }
    }

    #[test]
    fn ring_across_threads() {
        let r = Arc::new(MidiRing::new(64));
        let w = Arc::clone(&r);
        let t = std::thread::spawn(move || {
            let mut i = 0u64;
            while i < 10_000 {
                if w.push(i, i ^ 0xFF) {
                    i += 1;
                }
            }
        });
        let mut next = 0u64;
        while next < 10_000 {
            if let Some((a, b)) = r.pop() {
                assert_eq!((a, b), (next, next ^ 0xFF));
                next += 1;
            }
        }
        t.join().unwrap();
    }

    #[test]
    fn messages_parsed_and_filtered() {
        assert_eq!(parse(&[0x90, 60, 100]), Some([0x90, 60, 100]));
        assert_eq!(parse(&[0xC3, 5]), Some([0xC3, 5, 0]));
        assert_eq!(parse(&[0xD0, 0x7F]), Some([0xD0, 0x7F, 0]));
        assert_eq!(parse(&[0xE0, 0, 64]), Some([0xE0, 0, 64]));
        assert_eq!(parse(&[0xB0, 64, 127]), Some([0xB0, 64, 127]));
        for junk in [&[0xF8u8][..], &[0xFE], &[0xF0, 1, 2, 0xF7], &[60, 100], &[0x90, 60], &[]] {
            assert_eq!(parse(junk), None, "{junk:?}");
        }
    }

    #[test]
    fn offsets_one_block_behind_arrival() {
        // 48 kHz, 480-frame blocks (10 ms), rendered at t = 1 s.
        let now = 1_000_000;
        assert_eq!(block_offset(now - 10_000, now, 480, 48_000.0), 0, "a block ago: the block's start");
        assert_eq!(block_offset(now - 5_000, now, 480, 48_000.0), 240);
        assert_eq!(block_offset(now, now, 480, 48_000.0), 479, "just now: the block's end");
        assert_eq!(block_offset(now - 500_000, now, 480, 48_000.0), 0, "late: offset 0");
        assert_eq!(block_offset(now + 50, now, 480, 48_000.0), 479);
        assert_eq!(block_offset(now, now, 0, 48_000.0), 0);
    }

    #[test]
    fn hub_drains_ports_into_block_order() {
        let hub = MidiHub::new();
        let mut live = LiveMidi::new();
        let before = live.storage();
        hub.set_slot(0, Some("Keys"));
        hub.set_slot(1, Some("DIN"));
        let (keys, din) = (hub.sink(0), hub.sink(1));
        let now = 1_000_000;
        keys.send_at(now - 2_000, &[0x90, 60, 100]);
        din.send_at(now - 8_000, &[0xB0, 64, 127]);
        keys.send_at(now - 1_000, &[0x80, 60, 0]);
        keys.send_at(now - 1_000, &[0xF8]);
        hub.drain_into(&mut live, now, 480, 48_000.0);
        let got: Vec<(u32, u8, [u8; 3])> = live.events().iter().map(|e| (e.offset, e.port, e.msg)).collect();
        assert_eq!(got, vec![(96, 1, [0xB0, 64, 127]), (384, 0, [0x90, 60, 100]), (432, 0, [0x80, 60, 0])]);
        assert_eq!(live.ports[0], port_id("Keys"));
        assert_eq!(live.storage(), before);
        // A closed slot's messages are dropped.
        hub.set_slot(1, None);
        din.send_at(now, &[0x90, 1, 1]);
        hub.drain_into(&mut live, now, 480, 48_000.0);
        assert!(live.is_empty());
        assert!(hub.last_activity(1) > 0);
    }

    #[test]
    fn full_port_ring_drops_and_counts() {
        let hub = MidiHub::new();
        hub.set_slot(0, Some("Keys"));
        let k = hub.sink(0);
        for _ in 0..PORT_RING + 3 {
            k.send_at(5, &[0x90, 60, 1]);
        }
        assert_eq!(hub.dropped(), 3);
        let mut live = LiveMidi::new();
        hub.drain_into(&mut live, 10, 64, 48_000.0);
        assert_eq!(live.events().len(), PORT_RING);
    }

    #[test]
    fn recorded_events_come_back_with_position_and_pass() {
        let hub = MidiHub::new();
        let mut live = LiveMidi::new();
        // Stamped 2 ms before the callback's start (96 samples at 48 kHz), and 1 ms after it.
        live.push(LiveEvent { offset: 7, port: 2, msg: [0x91, 64, 90], stamp: 8_000 });
        live.push(LiveEvent { offset: 0, port: 2, msg: [0x81, 64, 0], stamp: 11_000 });
        hub.record_block(&live, 48_000, 10_000, 48_000.0, 3, i64::MIN);
        let mut out = Vec::new();
        hub.take_recorded(&mut out);
        assert_eq!(
            out,
            vec![
                RecordedMidi { at: 47_904, port: 2, msg: [0x91, 64, 90], pass: 3 },
                RecordedMidi { at: 48_048, port: 2, msg: [0x81, 64, 0], pass: 3 }
            ]
        );
        // With a floor (the take's start): nothing before it.
        hub.record_block(&live, 48_000, 10_000, 48_000.0, 0, 48_000);
        out.clear();
        hub.take_recorded(&mut out);
        assert_eq!(out.iter().map(|x| x.at).collect::<Vec<_>>(), vec![48_000, 48_048]);
    }

    #[test]
    fn what_is_heard_while_playing_is_captured_apart_from_a_take() {
        let hub = MidiHub::new();
        let mut live = LiveMidi::new();
        live.push(LiveEvent { offset: 0, port: 1, msg: [0x90, 60, 90], stamp: 9_000 });
        hub.capture_block(&live, 48_000, 10_000, 48_000.0, 2);
        let (mut rec, mut cap) = (Vec::new(), Vec::new());
        hub.take_recorded(&mut rec);
        hub.take_captured(&mut cap);
        assert!(rec.is_empty(), "not a take's");
        assert_eq!(cap, vec![RecordedMidi { at: 47_952, port: 1, msg: [0x90, 60, 90], pass: 2 }]);
    }

    #[test]
    fn what_is_played_with_the_transport_stopped_is_heard_on_the_listening_clock() {
        let hub = MidiHub::new();
        let mut live = LiveMidi::new();
        live.push(LiveEvent { offset: 0, port: 1, msg: [0x90, 60, 90], stamp: 1_000_000 });
        live.push(LiveEvent { offset: 0, port: 1, msg: [0x80, 60, 0], stamp: 1_500_000 });
        hub.listen_block(&live, 48_000.0);
        let mut cap = Vec::new();
        hub.take_captured(&mut cap);
        assert_eq!(cap.iter().map(|x| (x.at, x.pass)).collect::<Vec<_>>(), vec![(48_000, LISTENING), (72_000, LISTENING)]);
    }

    #[test]
    fn the_input_filter_leaves_out_the_kinds_chosen() {
        let hub = MidiHub::new();
        let keys = hub.typing();
        hub.set_filter(FILTER_KINDS.iter().filter(|(k, _)| matches!(*k, "poly_aftertouch" | "cc")).map(|(_, b)| b).sum());
        keys.send_at(5, &[0x90, 60, 100]);
        keys.send_at(6, &[0xA0, 60, 40]);
        keys.send_at(7, &[0xB0, 1, 64]);
        keys.send_at(8, &[0xE0, 0, 70]);
        let mut live = LiveMidi::new();
        hub.drain_into(&mut live, 10, 64, 48_000.0);
        assert_eq!(live.events().iter().map(|e| e.msg[0]).collect::<Vec<_>>(), vec![0x90, 0xE0]);
        assert!(hub.last_activity(TYPING_SLOT) >= 7, "the light still shows it came");
    }

    #[test]
    fn a_pedal_down_before_the_take_is_down_in_it() {
        let hub = MidiHub::new();
        let keys = hub.typing();
        keys.send_at(5, &[0xB2, 64, 127]);
        keys.send_at(6, &[0xB0, 66, 100]);
        keys.send_at(7, &[0xB0, 64, 127]);
        keys.send_at(8, &[0xB0, 64, 0]);
        hub.discard();
        hub.record_pedals(4_800, 0);
        let mut out = Vec::new();
        hub.take_recorded(&mut out);
        let got: Vec<(i64, u8, [u8; 3])> = out.iter().map(|x| (x.at, x.port, x.msg)).collect();
        let slot = TYPING_SLOT as u8;
        assert_eq!(got, vec![(4_800, slot, [0xB2, 64, 127]), (4_800, slot, [0xB0, 66, 127])]);
    }

    #[test]
    fn inputs_open_enabled_ports_and_follow_hot_plug() {
        let fake = FakeBackend::with_ports(&["Keys", "Focusrite USB MIDI"]);
        let hub = MidiHub::new();
        let mut choices = BTreeMap::new();
        choices.insert("Focusrite USB MIDI".to_string(), false);
        let mut m = MidiInputs::new(fake.boxed(), Arc::clone(&hub), choices);
        let t0 = Instant::now();
        m.tick(t0);
        assert!(fake.is_open("Keys") && !fake.is_open("Focusrite USB MIDI"));
        let ports = m.ports();
        assert_eq!(ports.iter().map(|p| (p.name.as_str(), p.open)).collect::<Vec<_>>(), vec![("Keys", true), ("Focusrite USB MIDI", false)]);
        // Playing the keyboard reaches the ring and lights its activity.
        assert!(fake.send("Keys", &[0x90, 60, 100]));
        assert!(m.since_activity("Keys").is_some());
        let mut live = LiveMidi::new();
        hub.drain_into(&mut live, hub.now_us(), 256, 48_000.0);
        assert_eq!(live.events().len(), 1);

        // Enabling opens it at once; the choice is kept by name.
        m.set_enabled("Focusrite USB MIDI", true);
        assert!(fake.is_open("Focusrite USB MIDI"));
        assert_eq!(m.choices().get("Focusrite USB MIDI"), Some(&true));

        // Unplugged: closed at the next scan (2 s on), not before.
        fake.unplug("Keys");
        m.tick(t0 + Duration::from_millis(500));
        assert!(fake.is_open("Keys"));
        m.tick(t0 + RESCAN_EVERY);
        assert!(!fake.is_open("Keys"));
        assert!(m.ports().iter().any(|p| p.name == "Keys" && !p.present && !p.open));
        assert!(!hub.any_open() || live.ports[0] != 0);
        // Back: reopened in the same slot.
        fake.plug("Keys");
        m.rescan();
        assert!(fake.is_open("Keys"));
        assert_eq!(m.ports().iter().find(|p| p.name == "Keys").map(|p| p.slot), Some(0));

        // A port that refuses to open says why and stays closed.
        fake.plug("Busy");
        fake.refuse("Busy");
        m.rescan();
        let busy = m.ports().into_iter().find(|p| p.name == "Busy").unwrap();
        assert!(!busy.open && busy.error.is_some());

        // Hardware never takes Musical Typing's slot, however many ports come.
        for i in 0..20 {
            fake.plug(&format!("Port {i}"));
        }
        m.rescan();
        assert!(m.ports().iter().all(|p| p.slot < TYPING_SLOT));
    }

    #[test]
    fn identical_devices_more_than_fifteen_ports_and_a_quick_replug() {
        let fake = FakeBackend::with_ports(&["Keys", "Keys"]);
        let hub = MidiHub::new();
        let mut m = MidiInputs::new(fake.boxed(), Arc::clone(&hub), BTreeMap::new());
        m.rescan();
        // Two identical keyboards: both usable, the second as `Keys #2`.
        assert!(fake.is_open("Keys") && fake.is_open("Keys #2"));
        assert!(fake.send("Keys #2", &[0x90, 60, 100]));
        let mut live = LiveMidi::new();
        hub.drain_into(&mut live, hub.now_us(), 64, 48_000.0);
        assert_eq!(live.ports[1], port_id("Keys #2"));
        // Plugged out and in between two scans: the next block lets go of what it held.
        fake.replug("Keys #2");
        m.rescan();
        assert!(fake.is_open("Keys #2"), "open again");
        hub.drain_into(&mut live, hub.now_us(), 64, 48_000.0);
        assert_eq!(live.reset, 1 << 1);
        hub.drain_into(&mut live, hub.now_us(), 64, 48_000.0);
        assert_eq!(live.reset, 0, "once");
        // Forty ports all open (hardware never takes Musical Typing's slot).
        for i in 0..40 {
            fake.plug(&format!("Port {i}"));
        }
        m.rescan();
        assert!((0..40).all(|i| fake.is_open(&format!("Port {i}"))));
    }

    #[test]
    fn a_device_offset_places_its_messages_earlier_and_the_driver_clock_keeps_their_spacing() {
        let fake = FakeBackend::with_ports(&["Slow Keys"]);
        let hub = MidiHub::new();
        let mut m = MidiInputs::new(fake.boxed(), Arc::clone(&hub), BTreeMap::new());
        m.rescan();
        m.set_offsets(BTreeMap::from([("Slow Keys".to_string(), 5.0)]));
        let now = hub.now_us() + 20_000;
        hub.sink(0).send_at(now, &[0x90, 60, 100]);
        let mut live = LiveMidi::new();
        hub.drain_into(&mut live, now, 480, 48_000.0);
        // 5 ms early: 240 samples before the block's end.
        assert_eq!(live.events()[0].offset, 480 - 240);
        // The driver's stamps 1 ms apart, delivered together late: kept 1 ms apart.
        let mut c = DriverClock::default();
        assert_eq!(c.to_hub(1_000, 50_000), 50_000);
        assert_eq!(c.to_hub(2_000, 52_000), 51_000);
        assert_eq!(c.to_hub(3_000, 52_000), 52_000);
    }

    #[test]
    fn held_notes_follow_note_on_and_off() {
        let hub = MidiHub::new();
        let (a, b) = (hub.typing(), hub.sink(0));
        a.send(&[0x90, 64, 100]);
        b.send(&[0x91, 57, 100]);
        a.send(&[0x90, 100, 1]);
        assert_eq!(hub.held_notes(), vec![57, 64, 100]);
        a.send(&[0x80, 100, 0]);
        b.send(&[0x91, 57, 0]);
        assert_eq!(hub.held_notes(), vec![64]);
        a.send(&[0xB0, 123, 0]);
        assert!(hub.held_notes().is_empty());
    }

    #[test]
    fn musical_typing_is_a_port() {
        let hub = MidiHub::new();
        let t = hub.typing();
        t.send(&[0x90, 60, 100]);
        let mut live = LiveMidi::new();
        hub.drain_into(&mut live, hub.now_us(), 128, 48_000.0);
        assert_eq!(live.ports[TYPING_SLOT], port_id(TYPING_NAME));
        assert_eq!(live.events().first().map(|e| e.port as usize), Some(TYPING_SLOT));
    }
}
