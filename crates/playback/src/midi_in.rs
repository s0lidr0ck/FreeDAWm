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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
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
    record: MidiRing,
    /// The notes held now on any port and channel (bit per pitch), for naming a chord as it is
    /// played (the Chords ruler's picker).
    held: [AtomicU64; 2],
}

impl MidiHub {
    pub fn new() -> Arc<MidiHub> {
        Arc::new(MidiHub {
            epoch: Instant::now(),
            rings: std::array::from_fn(|_| MidiRing::new(PORT_RING)),
            ids: std::array::from_fn(|_| AtomicU64::new(0)),
            activity: std::array::from_fn(|_| AtomicU64::new(0)),
            thru: AtomicBool::new(true),
            record: MidiRing::new(RECORD_RING),
            held: [AtomicU64::new(0), AtomicU64::new(0)],
        })
    }

    /// Microseconds since the hub was made (never 0, which means "never").
    pub fn now_us(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_micros()).unwrap_or(u64::MAX).max(1)
    }

    /// MIDI Thru: record-armed instrument and MIDI tracks play their live input.
    pub fn set_thru(&self, on: bool) {
        self.thru.store(on, Ordering::Relaxed);
    }

    pub fn thru(&self) -> bool {
        self.thru.load(Ordering::Relaxed)
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
        for e in live.events() {
            let age = ((now as f64 - e.stamp as f64) * rate / 1e6).round() as i64;
            let at = base.saturating_sub(age).max(floor);
            let packed = pack(e.msg) | (u64::from(e.port) << 24) | (u64::from(pass) << 32);
            self.record.push(at as u64, packed);
        }
    }

    /// UI thread: what was recorded since the last call.
    pub fn take_recorded(&self, out: &mut Vec<RecordedMidi>) {
        while let Some((at, packed)) = self.record.pop() {
            out.push(RecordedMidi {
                at: at as i64,
                port: ((packed >> 24) & 0xFF) as u8,
                msg: unpack(packed),
                pass: ((packed >> 32) & 0xFFFF) as u16,
            });
        }
    }

    /// Recorded events lost because the UI thread fell behind.
    pub fn record_dropped(&self) -> u64 {
        self.record.dropped()
    }
}

/// The sending end of one port slot's ring, handed to the port's driver callback.
pub struct PortSink {
    hub: Arc<MidiHub>,
    slot: usize,
}

impl PortSink {
    /// One raw message from the port, stamped now. What the mix doesn't play is ignored.
    pub fn send(&self, bytes: &[u8]) {
        self.send_at(self.hub.now_us(), bytes);
    }

    /// [`PortSink::send`] stamped `stamp` (µs since the hub was made).
    pub fn send_at(&self, stamp: u64, bytes: &[u8]) {
        let Some(msg) = parse(bytes) else { return };
        if let Some(a) = self.hub.activity.get(self.slot) {
            a.store(stamp.max(1), Ordering::Relaxed);
        }
        self.hub.note_held(msg);
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
}

/// The UI thread's MIDI input ports (see the module docs).
pub struct MidiInputs {
    backend: Box<dyn MidiBackend>,
    hub: Arc<MidiHub>,
    ports: Vec<Port>,
    /// Enabled (true) or disabled by port name, as saved; a port not named is enabled.
    choices: BTreeMap<String, bool>,
    last_scan: Option<Instant>,
}

impl MidiInputs {
    pub fn new(backend: Box<dyn MidiBackend>, hub: Arc<MidiHub>, choices: BTreeMap<String, bool>) -> MidiInputs {
        MidiInputs { backend, hub, ports: Vec::new(), choices, last_scan: None }
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
        for p in &mut self.ports {
            p.info.present = names.contains(&p.info.name);
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
            self.ports.push(Port { info: PortInfo { name: n, enabled: true, present: true, open: false, error: None, slot }, conn: None });
        }
        self.apply();
    }

    /// Opens what should be open and closes the rest.
    fn apply(&mut self) {
        for i in 0..self.ports.len() {
            let Some(p) = self.ports.get(i) else { continue };
            let enabled = self.enabled(&p.info.name);
            let want = enabled && p.info.present;
            let (name, slot, has) = (p.info.name.clone(), p.info.slot, p.conn.is_some());
            let mut result = None;
            if want && !has {
                // The slot starts empty: whatever an old port left in it is not this port's.
                self.hub.set_slot(slot, Some(&name));
                result = Some(self.backend.open(&name, self.hub.sink(slot)));
            }
            let Some(p) = self.ports.get_mut(i) else { continue };
            p.info.enabled = enabled;
            match result {
                Some(Ok(c)) => {
                    p.conn = Some(c);
                    p.info.error = None;
                }
                Some(Err(err)) => {
                    log::warn!("MIDI input {name}: {err}");
                    p.info.error = Some(err);
                    self.hub.set_slot(slot, None);
                }
                None if !want => {
                    // Closing drops the connection: the driver stops calling back.
                    p.conn = None;
                    self.hub.set_slot(slot, None);
                }
                None => {}
            }
            p.info.open = p.conn.is_some();
        }
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
        m.ignore(midir::Ignore::All);
        Ok(m)
    }

    impl MidiBackend for Midir {
        fn ports(&mut self) -> Vec<String> {
            let Ok(m) = client() else { return Vec::new() };
            let mut out: Vec<String> = m.ports().iter().filter_map(|p| m.port_name(p).ok()).collect();
            out.dedup();
            out
        }

        fn open(&mut self, name: &str, sink: PortSink) -> Result<Box<dyn MidiConnection>, String> {
            let m = client()?;
            let port = m.ports().into_iter().find(|p| m.port_name(p).ok().as_deref() == Some(name)).ok_or_else(|| "not found".to_string())?;
            let conn = m.connect(&port, "FreeDAWm input", move |_stamp, bytes, _| sink.send(bytes), ()).map_err(|e| e.to_string())?;
            Ok(Box::new(Conn(conn)))
        }
    }
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
        self.state.lock().map(|s| s.ports.clone()).unwrap_or_default()
    }

    fn open(&mut self, name: &str, sink: PortSink) -> Result<Box<dyn MidiConnection>, String> {
        let mut s = self.state.lock().map_err(|_| "poisoned".to_string())?;
        if s.refuse.iter().any(|r| r == name) {
            return Err("in use by another program".into());
        }
        if !s.ports.iter().any(|p| p == name) {
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
