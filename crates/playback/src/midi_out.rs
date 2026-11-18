//! Hardware MIDI output ports: what External MIDI instruments play
//! (`soundcraft_mix::external_midi`) leaves through them.
//!
//! - [`MidiOutputs`]: the UI thread's list of output ports, through a [`MidiOutBackend`] (the
//!   system's, `midir`, native only; or a fake for tests). Rescanned when asked and every 2 s;
//!   enabled ports are kept by name in the app settings. A port never chosen is disabled (on
//!   Windows a port can be open in one program at a time: FreeDAWm takes none it was not asked
//!   for) unless the session's External MIDI instruments name it ([`MidiOutputs::set_wanted`]);
//!   an enabled port that is present is open.
//! - Each open port has a sender thread: it takes the port's slot in the process's
//!   [`soundcraft_mix::external_midi::OutHub`], empties the slot's ring about every half
//!   millisecond and sends each message when its time comes (the audio thread only pushes into
//!   the ring: it never blocks on the driver). The thread remembers the notes it left on.
//! - Closing a port (disabled, unplugged, the app quitting) lets go of what it left on: a
//!   note-off for each held note, then sustain up and All Notes Off on every channel, before the
//!   port closes.

use soundcraft_mix::external_midi::{self, OutHub, OutMsg, message_len};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How often a sender looks at its ring.
const SENDER_TICK: Duration = Duration::from_micros(500);

/// An open output port: sends raw MIDI bytes.
pub trait MidiSend: Send {
    /// Sends one message now; false when the driver refused it.
    fn send(&mut self, bytes: &[u8]) -> bool;
}

/// Where output ports come from: the system's (`midir`), or a fake for tests.
pub trait MidiOutBackend: Send {
    /// The output ports there are now, by name.
    fn ports(&mut self) -> Vec<String>;
    /// Opens the port named `name`.
    fn open(&mut self, name: &str) -> Result<Box<dyn MidiSend>, String>;
}

/// One output port as Setup › MIDI Devices lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutPortInfo {
    pub name: String,
    pub enabled: bool,
    /// Present on the system now.
    pub present: bool,
    /// Open (present, enabled and opened).
    pub open: bool,
    /// Why it could not open.
    pub error: Option<String>,
    /// Used by a control surface (opened by it, not here).
    pub reserved: bool,
    /// A track of the session sends to it (an External MIDI instrument names it).
    pub wanted: bool,
}

struct Sender {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// Whole messages from the UI thread, sent as they come (MIDI Machine Control out).
    now: std::sync::mpsc::Sender<Vec<u8>>,
}

impl Drop for Sender {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Port {
    info: OutPortInfo,
    sender: Option<Sender>,
}

/// The UI thread's MIDI output ports (see the module docs).
pub struct MidiOutputs {
    backend: Box<dyn MidiOutBackend>,
    hub: &'static OutHub,
    ports: Vec<Port>,
    choices: BTreeMap<String, bool>,
    reserved: Vec<String>,
    /// The ports the session's tracks send to.
    wanted: Vec<String>,
    last_scan: Option<Instant>,
}

impl MidiOutputs {
    pub fn new(backend: Box<dyn MidiOutBackend>, choices: BTreeMap<String, bool>) -> MidiOutputs {
        MidiOutputs { backend, hub: external_midi::hub(), ports: Vec::new(), choices, reserved: Vec::new(), wanted: Vec::new(), last_scan: None }
    }

    /// The system's ports (`None` where there is no MIDI backend).
    pub fn system(choices: BTreeMap<String, bool>) -> Option<MidiOutputs> {
        system_backend().map(|b| MidiOutputs::new(b, choices))
    }

    /// Every port seen since the app started, in the order first seen.
    pub fn ports(&self) -> Vec<OutPortInfo> {
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

    /// Whether `name` is enabled: chosen so, or never chosen and a track sends to it.
    pub fn enabled(&self, name: &str) -> bool {
        self.choices.get(name).copied().unwrap_or_else(|| self.wanted.iter().any(|w| w == name))
    }

    /// The ports the session's tracks send to (External MIDI instruments): one never chosen in
    /// the window opens for them; one disabled there stays closed (the window says a track wants it).
    pub fn set_wanted(&mut self, names: Vec<String>) {
        if names != self.wanted {
            self.wanted = names;
            self.apply();
        }
    }

    /// Enables or disables the port named `name`, opening or closing it now.
    pub fn set_enabled(&mut self, name: &str, on: bool) {
        self.choices.insert(name.to_string(), on);
        self.apply();
    }

    /// The ports control surfaces use: they are closed here (a surface opens its own).
    pub fn set_reserved(&mut self, names: Vec<String>) {
        if names != self.reserved {
            self.reserved = names;
            self.apply();
        }
    }

    /// Sends `bytes` (one whole message, SysEx too) to the port named `name` at once, from the UI
    /// thread, not timed with the audio (MIDI Machine Control out); false when it is not open.
    pub fn send_now(&self, name: &str, bytes: &[u8]) -> bool {
        self.ports.iter().find(|p| p.info.name == name).and_then(|p| p.sender.as_ref()).is_some_and(|s| s.now.send(bytes.to_vec()).is_ok())
    }

    /// Rescans when the last scan is 2 s old (or there was none).
    pub fn tick(&mut self, now: Instant) {
        if self.last_scan.is_none_or(|t| now.saturating_duration_since(t) >= crate::midi_in::RESCAN_EVERY) {
            self.last_scan = Some(now);
            self.rescan_now();
        }
    }

    /// Rescans the ports now.
    pub fn rescan(&mut self) {
        self.last_scan = Some(Instant::now());
        self.rescan_now();
    }

    fn rescan_now(&mut self) {
        let names = self.backend.ports();
        for p in &mut self.ports {
            p.info.present = names.contains(&p.info.name);
        }
        for n in names {
            if !self.ports.iter().any(|p| p.info.name == n) {
                self.ports.push(Port {
                    info: OutPortInfo { name: n, enabled: false, present: true, open: false, error: None, reserved: false, wanted: false },
                    sender: None,
                });
            }
        }
        self.apply();
    }

    fn apply(&mut self) {
        for i in 0..self.ports.len() {
            let Some(p) = self.ports.get(i) else { continue };
            let name = p.info.name.clone();
            let enabled = self.enabled(&name);
            let reserved = self.reserved.contains(&name);
            let want = enabled && p.info.present && !reserved;
            let has = p.sender.is_some();
            let opened = (want && !has).then(|| self.open(&name));
            let Some(p) = self.ports.get_mut(i) else { continue };
            p.info.enabled = enabled;
            p.info.reserved = reserved;
            p.info.wanted = self.wanted.contains(&name);
            match opened {
                Some(Ok(s)) => {
                    p.sender = Some(s);
                    p.info.error = None;
                }
                Some(Err(e)) => {
                    log::warn!("MIDI output {name}: {e}");
                    p.info.error = Some(e);
                }
                None if !want => p.sender = None,
                None => {}
            }
            p.info.open = p.sender.is_some();
        }
    }

    fn open(&mut self, name: &str) -> Result<Sender, String> {
        let conn = self.backend.open(name)?;
        let slot = self.hub.open(name).ok_or_else(|| "too many MIDI output ports open".to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let (hub, flag) = (self.hub, Arc::clone(&stop));
        let (now, rx) = std::sync::mpsc::channel();
        let thread =
            std::thread::Builder::new().name(format!("MIDI out {name}")).spawn(move || run_sender(hub, slot, conn, &flag, &rx)).map_err(|e| {
                hub.close(slot);
                e.to_string()
            })?;
        Ok(Sender { stop, thread: Some(thread), now })
    }
}

/// A sender thread: sends what is queued for `slot` when it is due, until told to stop; then
/// lets go of every note it left on and gives the slot back.
fn run_sender(hub: &'static OutHub, slot: usize, mut conn: Box<dyn MidiSend>, stop: &AtomicBool, now_rx: &std::sync::mpsc::Receiver<Vec<u8>>) {
    let mut pending: Vec<(u64, OutMsg)> = Vec::with_capacity(external_midi::OUT_RING);
    let mut held = [[0u64; 2]; 16];
    let send = |conn: &mut Box<dyn MidiSend>, held: &mut [[u64; 2]; 16], m: OutMsg| {
        let m = match m {
            OutMsg::Short(m) => m,
            OutMsg::SysEx(bytes) => {
                conn.send(&bytes);
                return;
            }
        };
        let (ch, pitch) = (usize::from(m[0] & 0x0F), m[1] & 0x7F);
        if let Some(w) = held.get_mut(ch).and_then(|h| h.get_mut(usize::from(pitch / 64))) {
            let bit = 1u64 << (pitch % 64);
            match m[0] & 0xF0 {
                0x90 if m[2] > 0 => *w |= bit,
                0x80 | 0x90 => *w &= !bit,
                _ => {}
            }
        }
        let n = message_len(m[0]).min(m.len());
        conn.send(m.get(..n).unwrap_or(&m));
    };
    loop {
        let stopping = stop.load(Ordering::Acquire);
        while let Some(e) = hub.pop(slot) {
            if pending.len() < pending.capacity() {
                pending.push(e);
            }
        }
        if stopping {
            // What was queued and not yet due is dropped, except the note-offs (a stop's).
            for (_, m) in pending.drain(..) {
                if let OutMsg::Short(s) = m
                    && (s[0] & 0xF0 == 0x80 || (s[0] & 0xF0 == 0x90 && s[2] == 0))
                {
                    send(&mut conn, &mut held, m);
                }
            }
            break;
        }
        // Stable: messages due together keep the order they were played in.
        pending.sort_by_key(|e| e.0);
        while let Ok(bytes) = now_rx.try_recv() {
            conn.send(&bytes);
        }
        let now = hub.now_us();
        let due = pending.partition_point(|e| e.0 <= now);
        for (_, m) in pending.drain(..due) {
            send(&mut conn, &mut held, m);
        }
        std::thread::sleep(SENDER_TICK);
    }
    hub.close(slot);
    while hub.pop(slot).is_some() {}
    for (ch, words) in held.iter().enumerate() {
        let ch = ch as u8;
        for (w, bits) in words.iter().enumerate() {
            for k in 0..64u8 {
                if bits & (1u64 << k) != 0 {
                    conn.send(&[0x80 | ch, (w as u8) * 64 + k, 0]);
                }
            }
        }
        conn.send(&[0xB0 | ch, 64, 0]);
        conn.send(&[0xB0 | ch, 123, 0]);
    }
}

mod system {
    use super::{MidiOutBackend, MidiSend};

    pub struct Midir;

    struct Conn(midir::MidiOutputConnection);

    impl MidiSend for Conn {
        fn send(&mut self, bytes: &[u8]) -> bool {
            self.0.send(bytes).is_ok()
        }
    }

    fn client() -> Result<midir::MidiOutput, String> {
        midir::MidiOutput::new("FreeDAWm").map_err(|e| e.to_string())
    }

    /// The system's output port names.
    pub fn port_names() -> Vec<String> {
        let Ok(m) = client() else { return Vec::new() };
        let mut out: Vec<String> = m.ports().iter().filter_map(|p| m.port_name(p).ok()).collect();
        out.dedup();
        out
    }

    /// Opens the system's output port `name`.
    pub fn open_port(name: &str) -> Result<Box<dyn MidiSend>, String> {
        let m = client()?;
        let port = m.ports().into_iter().find(|p| m.port_name(p).ok().as_deref() == Some(name)).ok_or_else(|| "not found".to_string())?;
        let conn = m.connect(&port, "FreeDAWm output").map_err(|e| e.to_string())?;
        Ok(Box::new(Conn(conn)))
    }

    impl MidiOutBackend for Midir {
        fn ports(&mut self) -> Vec<String> {
            port_names()
        }

        fn open(&mut self, name: &str) -> Result<Box<dyn MidiSend>, String> {
            open_port(name)
        }
    }
}

/// The system's MIDI output backend.
pub fn system_backend() -> Option<Box<dyn MidiOutBackend>> {
    Some(Box::new(system::Midir))
}

/// Opens the system's output port `name` directly (a control surface's feedback port).
pub fn open_system_port(name: &str) -> Result<Box<dyn MidiSend>, String> {
    system::open_port(name)
}

/// The system's output port names.
pub fn system_port_names() -> Vec<String> {
    system::port_names()
}

/// A backend with ports a test adds and removes; what each port was sent is kept. Shared state,
/// so a test keeps a handle while [`MidiOutputs`] owns the backend.
#[derive(Clone, Default)]
pub struct FakeOutBackend {
    state: Arc<std::sync::Mutex<FakeOut>>,
}

#[derive(Default)]
struct FakeOut {
    ports: Vec<String>,
    sent: Vec<(String, Vec<u8>)>,
    open: Vec<String>,
}

struct FakeSend {
    state: Arc<std::sync::Mutex<FakeOut>>,
    name: String,
}

impl MidiSend for FakeSend {
    fn send(&mut self, bytes: &[u8]) -> bool {
        self.state.lock().map(|mut s| s.sent.push((self.name.clone(), bytes.to_vec()))).is_ok()
    }
}

impl Drop for FakeSend {
    fn drop(&mut self) {
        if let Ok(mut s) = self.state.lock()
            && let Some(i) = s.open.iter().position(|n| *n == self.name)
        {
            s.open.remove(i);
        }
    }
}

impl FakeOutBackend {
    pub fn with_ports(names: &[&str]) -> FakeOutBackend {
        let f = FakeOutBackend::default();
        if let Ok(mut s) = f.state.lock() {
            s.ports = names.iter().map(|n| n.to_string()).collect();
        }
        f
    }

    /// Unplugs the port named `name`.
    pub fn unplug(&self, name: &str) {
        if let Ok(mut s) = self.state.lock() {
            s.ports.retain(|p| p != name);
        }
    }

    /// Whether `name` is open.
    pub fn is_open(&self, name: &str) -> bool {
        self.state.lock().is_ok_and(|s| s.open.iter().any(|n| n == name))
    }

    /// What was sent to `name`, oldest first.
    pub fn sent(&self, name: &str) -> Vec<Vec<u8>> {
        self.state.lock().map(|s| s.sent.iter().filter(|(n, _)| n == name).map(|(_, b)| b.clone()).collect()).unwrap_or_default()
    }

    /// Opens `name` as a [`MidiSend`] (for a control surface's feedback in tests).
    pub fn open_send(&self, name: &str) -> Box<dyn MidiSend> {
        if let Ok(mut s) = self.state.lock() {
            s.open.push(name.to_string());
        }
        Box::new(FakeSend { state: Arc::clone(&self.state), name: name.to_string() })
    }

    pub fn boxed(&self) -> Box<dyn MidiOutBackend> {
        Box::new(self.clone())
    }
}

impl MidiOutBackend for FakeOutBackend {
    fn ports(&mut self) -> Vec<String> {
        self.state.lock().map(|s| s.ports.clone()).unwrap_or_default()
    }

    fn open(&mut self, name: &str) -> Result<Box<dyn MidiSend>, String> {
        if !self.state.lock().is_ok_and(|s| s.ports.iter().any(|p| p == name)) {
            return Err("not found".into());
        }
        Ok(self.open_send(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(f: impl Fn() -> bool) {
        let t = Instant::now();
        while !f() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn an_open_port_sends_what_is_due_and_lets_go_when_closed() {
        let fake = FakeOutBackend::with_ports(&["Out Test Synth"]);
        let mut outs = MidiOutputs::new(fake.boxed(), BTreeMap::new());
        outs.rescan();
        assert!(!fake.is_open("Out Test Synth"), "a port never chosen is disabled");
        outs.set_enabled("Out Test Synth", true);
        assert!(fake.is_open("Out Test Synth"));
        let hub = external_midi::hub();
        let key = external_midi::port_key("Out Test Synth");
        let now = hub.now_us();
        // Queued out of order: sent by time; one far in the future waits.
        assert!(hub.send(key, now + 2_000, [0x80, 60, 0]));
        assert!(hub.send(key, now, [0x90, 60, 100]));
        assert!(hub.send(key, now, [0xC1, 5, 0]));
        assert!(hub.send(key, now + 1_000, [0x90, 64, 90]));
        assert!(hub.send_sysex(key, now + 1_500, &[0xF0, 0x43, 0x10, 0xF7]));
        assert!(hub.send(key, now + 60_000_000, [0x90, 67, 90]));
        wait_for(|| fake.sent("Out Test Synth").len() >= 5);
        assert_eq!(
            fake.sent("Out Test Synth"),
            vec![vec![0x90, 60, 100], vec![0xC1, 5], vec![0x90, 64, 90], vec![0xF0, 0x43, 0x10, 0xF7], vec![0x80, 60, 0]],
            "a SysEx goes whole, in time order"
        );
        // Disabled: the held note gets its note-off, every channel sustain up and All Notes Off;
        // the note not yet due never goes.
        outs.set_enabled("Out Test Synth", false);
        assert!(!fake.is_open("Out Test Synth"));
        let sent = fake.sent("Out Test Synth");
        assert_eq!(sent.get(5), Some(&vec![0x80, 64, 0]));
        assert!(sent.contains(&vec![0xBF, 123, 0]) && sent.contains(&vec![0xB0, 64, 0]));
        assert!(!sent.contains(&vec![0x90, 67, 90]));
        assert_eq!(sent.len(), 6 + 32);
        assert!(!hub.send(key, 0, [0x90, 1, 1]), "the slot is free again");
        assert_eq!(outs.choices().get("Out Test Synth"), Some(&false));
    }

    #[test]
    fn a_whole_message_goes_out_at_once_from_the_ui() {
        let fake = FakeOutBackend::with_ports(&["Out Test Deck"]);
        let mut outs = MidiOutputs::new(fake.boxed(), BTreeMap::new());
        outs.rescan();
        let play = [0xF0, 0x7F, 0x7F, 0x06, 0x02, 0xF7];
        assert!(!outs.send_now("Out Test Deck", &play), "not open");
        outs.set_enabled("Out Test Deck", true);
        assert!(outs.send_now("Out Test Deck", &play));
        wait_for(|| !fake.sent("Out Test Deck").is_empty());
        assert_eq!(fake.sent("Out Test Deck"), vec![play.to_vec()]);
        outs.set_enabled("Out Test Deck", false);
    }

    #[test]
    fn a_port_never_chosen_opens_only_for_a_track_that_sends_to_it() {
        let fake = FakeOutBackend::with_ports(&["Out Test Juno", "Out Test Moog", "Out Test Off"]);
        let mut choices = BTreeMap::new();
        choices.insert("Out Test Moog".to_string(), true);
        choices.insert("Out Test Off".to_string(), false);
        let mut outs = MidiOutputs::new(fake.boxed(), choices);
        outs.rescan();
        assert!(!fake.is_open("Out Test Juno") && fake.is_open("Out Test Moog"), "enabled before: still open");
        // A session whose tracks play the Juno and the port disabled in the window.
        outs.set_wanted(vec!["Out Test Juno".into(), "Out Test Off".into()]);
        assert!(fake.is_open("Out Test Juno"));
        assert!(!fake.is_open("Out Test Off"), "a port disabled by choice stays closed");
        let off = outs.ports().into_iter().find(|p| p.name == "Out Test Off").unwrap();
        assert!(off.wanted && !off.enabled && !off.open, "the window can say a track wants it");
        // That session closed: the Juno closes again; nothing was saved for it.
        outs.set_wanted(Vec::new());
        assert!(!fake.is_open("Out Test Juno"));
        assert!(!outs.choices().contains_key("Out Test Juno"));
    }

    #[test]
    fn a_reserved_or_unplugged_port_is_closed() {
        let fake = FakeOutBackend::with_ports(&["Out Test Surface", "Out Test Keys"]);
        let mut outs = MidiOutputs::new(fake.boxed(), BTreeMap::new());
        outs.rescan();
        outs.set_reserved(vec!["Out Test Surface".into()]);
        assert!(!fake.is_open("Out Test Surface"));
        assert!(outs.ports().iter().any(|p| p.name == "Out Test Surface" && p.reserved && !p.open));
        fake.unplug("Out Test Keys");
        outs.rescan();
        assert!(!fake.is_open("Out Test Keys"));
        assert_eq!(outs.present(), vec!["Out Test Surface".to_string()]);
    }
}
