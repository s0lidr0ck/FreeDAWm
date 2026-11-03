//! Hardware MIDI input as the mix hears it: the events of one block, from every open input port.
//!
//! The audio host (`soundcraft-playback`'s `midi_in`) drains its per-port rings into a
//! [`LiveMidi`] at the start of each block, each event at its sample offset within the block,
//! sorted by offset (arrival order kept at equal offsets). Instrument and MIDI tracks whose input
//! is a MIDI input ([`soundcraft_model::Route::Midi`]) pick out their port and channel with a
//! [`MidiFilter`]. The buffer is made once with a fixed capacity: full, it drops and counts, and
//! nothing on the audio thread allocates.

/// Most MIDI input ports open at once (each has its own ring in the host).
pub const MAX_MIDI_PORTS: usize = 16;

/// Events one block holds, from all ports together.
pub const LIVE_MIDI_CAPACITY: usize = 2048;

/// One event of the block: `offset` samples into it, from port slot `port`, and when it arrived
/// (`stamp`, µs on the host's clock, 0 when not from the host: a recording places each event from
/// its own age, not its offset, which is clamped to the block).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LiveEvent {
    pub offset: u32,
    pub port: u8,
    pub msg: [u8; 3],
    pub stamp: u64,
}

/// The live MIDI of one block (see the module docs).
#[derive(Debug)]
pub struct LiveMidi {
    events: Vec<LiveEvent>,
    /// Room for [`LiveMidi::sort`]'s merges (same capacity, made once).
    scratch: Vec<LiveEvent>,
    /// Events that did not fit, since the buffer was made.
    pub dropped: u64,
    /// The identity ([`port_id`]) of the port in each slot, 0 for an empty slot.
    pub ports: [u64; MAX_MIDI_PORTS],
    /// MIDI Thru: record-armed tracks play what they are given (input-monitored ones always do).
    pub thru: bool,
}

impl Clone for LiveMidi {
    /// A copy with the full capacity (a derived clone's buffers would be only as big as their
    /// contents, and grow on the audio thread).
    fn clone(&self) -> Self {
        let mut c = LiveMidi::new();
        c.events.extend_from_slice(&self.events);
        (c.dropped, c.ports, c.thru) = (self.dropped, self.ports, self.thru);
        c
    }
}

impl Default for LiveMidi {
    fn default() -> Self {
        LiveMidi::new()
    }
}

impl LiveMidi {
    pub fn new() -> LiveMidi {
        LiveMidi {
            events: Vec::with_capacity(LIVE_MIDI_CAPACITY),
            scratch: Vec::with_capacity(LIVE_MIDI_CAPACITY),
            dropped: 0,
            ports: [0; MAX_MIDI_PORTS],
            thru: true,
        }
    }

    /// Empties the block (keeps the ports and Thru).
    pub fn clear(&mut self) {
        self.events.clear();
    }

    /// Adds an event; false (and counted) when the block is full.
    pub fn push(&mut self, e: LiveEvent) -> bool {
        if self.events.len() >= self.events.capacity().min(LIVE_MIDI_CAPACITY) {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        self.events.push(e);
        true
    }

    /// Orders the events by offset; events at the same offset keep the order they were pushed in
    /// (a ring's order). A stable merge of the sorted runs (each port's ring is one), through the
    /// scratch made with the buffer: O(n log n) however many ports interleave, no allocation.
    pub fn sort(&mut self) {
        let n = self.events.len();
        if n < 2 || !self.events.windows(2).any(|w| matches!(w, [a, b] if a.offset > b.offset)) {
            return;
        }
        // Bottom-up, back and forth between the buffers; the result ends in `events` (its address
        // and capacity kept).
        let mut in_scratch = false;
        let mut width = 1usize;
        while width < n {
            if in_scratch {
                merge_runs(&self.scratch, &mut self.events, width);
            } else {
                merge_runs(&self.events, &mut self.scratch, width);
            }
            in_scratch = !in_scratch;
            width = width.saturating_mul(2);
        }
        if in_scratch {
            self.events.clear();
            self.events.extend_from_slice(&self.scratch);
        }
    }

    pub fn events(&self) -> &[LiveEvent] {
        &self.events
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Whether any port is open (some slot has a port).
    pub fn any_port(&self) -> bool {
        self.ports.iter().any(|p| *p != 0)
    }

    /// Address and capacity of the event storage, to check the audio thread never reallocates it.
    pub fn storage(&self) -> (usize, usize) {
        (self.events.as_ptr() as usize, self.events.capacity())
    }
}

/// One pass of [`LiveMidi::sort`]: the runs of `width` in `src` merged in pairs into `dst`
/// (cleared first; its capacity is `src`'s, so it never grows). Ties take the left run's event.
fn merge_runs(src: &[LiveEvent], dst: &mut Vec<LiveEvent>, width: usize) {
    dst.clear();
    let n = src.len();
    let mut lo = 0;
    while lo < n {
        let mid = lo.saturating_add(width).min(n);
        let hi = lo.saturating_add(width.saturating_mul(2)).min(n);
        let (a, b) = (src.get(lo..mid).unwrap_or_default(), src.get(mid..hi).unwrap_or_default());
        let (mut i, mut j) = (0, 0);
        while let (Some(x), Some(y)) = (a.get(i), b.get(j)) {
            if y.offset < x.offset {
                dst.push(*y);
                j += 1;
            } else {
                dst.push(*x);
                i += 1;
            }
        }
        dst.extend_from_slice(a.get(i..).unwrap_or_default());
        dst.extend_from_slice(b.get(j..).unwrap_or_default());
        lo = hi;
    }
}

/// A stable identity for the port named `name` (FNV-1a; never 0, which marks an empty slot).
/// Tracks name their input port; the host stamps each slot with its port's identity, so the mix
/// matches them without comparing names.
pub fn port_id(name: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h.max(1)
}

/// Which live events a track takes: from one port (`None`: all inputs) and one channel (0: Omni,
/// else 1-16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MidiFilter {
    pub port: Option<u64>,
    pub channel: u8,
}

impl MidiFilter {
    /// The filter for a track input on `device` (`None`: all inputs) and `channel`.
    pub fn new(device: Option<&str>, channel: u8) -> MidiFilter {
        MidiFilter { port: device.map(port_id), channel: if channel > 16 { 0 } else { channel } }
    }

    /// Whether `e` passes: its port's identity matches (any port for all inputs) and, for a
    /// channel message, its channel (system messages pass on any channel).
    pub fn matches(&self, live: &LiveMidi, e: &LiveEvent) -> bool {
        if let Some(p) = self.port
            && live.ports.get(usize::from(e.port)).copied() != Some(p)
        {
            return false;
        }
        let status = e.msg[0];
        self.channel == 0 || status >= 0xF0 || (status & 0x0F) + 1 == self.channel
    }
}

/// Notes a live input holds on an instrument, by channel and pitch, and the channels it holds a
/// sustain or sostenuto pedal down on, each with the port slot it came from (fixed arrays: no
/// allocation), so they can be let go when the track stops hearing the input or the port closes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldNotes {
    notes: [u64; 32],
    /// The port slot of each held note (by channel × 128 + pitch).
    note_port: [u8; 2048],
    /// Bit per channel: the sustain pedal (CC 64), the sostenuto pedal (CC 66) down.
    pedals: [u16; 2],
    /// The port slot of each pedal down, by pedal and channel.
    pedal_port: [[u8; 16]; 2],
}

impl Default for HeldNotes {
    fn default() -> Self {
        HeldNotes { notes: [0; 32], note_port: [0; 2048], pedals: [0; 2], pedal_port: [[0; 16]; 2] }
    }
}

impl HeldNotes {
    /// Notes the message from port slot `port` on (a note-on with velocity, a pedal down) or off.
    pub fn track(&mut self, port: u8, msg: [u8; 3]) {
        let (kind, ch, d1) = (msg[0] & 0xF0, msg[0] & 0x0F, msg[1] & 0x7F);
        let chbit = 1u16 << ch;
        match kind {
            0xB0 => match d1 {
                64 | 66 => {
                    let k = usize::from(d1 == 66);
                    if let (Some(bits), Some(from)) = (self.pedals.get_mut(k), self.pedal_port.get_mut(k).and_then(|p| p.get_mut(usize::from(ch)))) {
                        if msg[2] >= 64 {
                            *bits |= chbit;
                            *from = port;
                        } else {
                            *bits &= !chbit;
                        }
                    }
                }
                // All Notes Off (the pedals stay), All Sound Off, Reset All Controllers.
                123 | 120 | 121 => {
                    if d1 != 121
                        && let Some(w) = self.notes.get_mut(usize::from(ch) * 2..usize::from(ch) * 2 + 2)
                    {
                        w.iter_mut().for_each(|x| *x = 0);
                    }
                    if d1 != 123 {
                        self.pedals.iter_mut().for_each(|b| *b &= !chbit);
                    }
                }
                _ => {}
            },
            0x80 | 0x90 => {
                let bit = usize::from(ch) * 128 + usize::from(d1);
                let Some(word) = self.notes.get_mut(bit / 64) else { return };
                if kind == 0x90 && msg[2] > 0 {
                    *word |= 1 << (bit % 64);
                    if let Some(p) = self.note_port.get_mut(bit) {
                        *p = port;
                    }
                } else {
                    *word &= !(1 << (bit % 64));
                }
            }
            _ => {}
        }
    }

    pub fn is_empty(&self) -> bool {
        self.notes.iter().all(|w| *w == 0) && self.pedals == [0; 2]
    }

    /// Calls `off` with a note-off for every held note, then a pedal-up for every pedal held
    /// down, and forgets them.
    pub fn release(&mut self, off: impl FnMut([u8; 3])) {
        self.release_ports(u16::MAX, off);
    }

    /// [`HeldNotes::release`] for what came from the port slots in `slots` (bit per slot).
    pub fn release_ports(&mut self, slots: u16, mut off: impl FnMut([u8; 3])) {
        let from = |p: u8| p < 16 && slots & (1 << p) != 0;
        for (w, word) in self.notes.iter_mut().enumerate() {
            let mut left = *word;
            while left != 0 {
                let b = left.trailing_zeros() as usize;
                left &= !(1u64 << b);
                let bit = w * 64 + b;
                if from(self.note_port.get(bit).copied().unwrap_or(0)) {
                    *word &= !(1u64 << b);
                    let (ch, pitch) = ((bit / 128) as u8, (bit % 128) as u8);
                    off([0x80 | (ch & 0x0F), pitch & 0x7F, 0]);
                }
            }
        }
        for ch in 0..16u8 {
            for (k, cc) in [(0usize, 64u8), (1, 66)] {
                let port = self.pedal_port.get(k).and_then(|p| p.get(usize::from(ch))).copied().unwrap_or(0);
                if let Some(bits) = self.pedals.get_mut(k)
                    && *bits & (1 << ch) != 0
                    && from(port)
                {
                    *bits &= !(1 << ch);
                    off([0xB0 | ch, cc, 0]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(offset: u32, port: u8, msg: [u8; 3]) -> LiveEvent {
        LiveEvent { offset, port, msg, stamp: 0 }
    }

    #[test]
    fn sorted_by_offset_keeping_arrival_order() {
        let mut m = LiveMidi::new();
        m.push(ev(10, 0, [0x90, 60, 1]));
        m.push(ev(3, 1, [0x90, 61, 1]));
        m.push(ev(10, 0, [0x80, 60, 0]));
        m.push(ev(0, 0, [0xB0, 64, 127]));
        m.sort();
        let got: Vec<(u32, u8)> = m.events().iter().map(|e| (e.offset, e.msg[1])).collect();
        assert_eq!(got, vec![(0, 64), (3, 61), (10, 60), (10, 60)]);
        assert_eq!(m.events()[2].msg[0], 0x90, "the note-on stays before its note-off");
    }

    #[test]
    fn interleaved_ports_sort_stably() {
        let mut m = LiveMidi::new();
        // 16 ports, each its own sorted run, interleaved by arrival (as `drain_into` pushes one
        // port's ring after another), filling the block.
        let per = LIVE_MIDI_CAPACITY / MAX_MIDI_PORTS;
        for port in 0..MAX_MIDI_PORTS {
            for i in 0..per {
                m.push(ev((i * 7 + port) as u32 % 1000, port as u8, [0x90, i as u8 & 0x7F, 1]));
            }
        }
        let mut want: Vec<LiveEvent> = m.events().to_vec();
        want.sort_by_key(|e| e.offset);
        let before = m.storage();
        m.sort();
        assert_eq!(m.events(), want.as_slice());
        assert_eq!(m.storage(), before, "never reallocated");
        m.clear();
        for i in 0..LIVE_MIDI_CAPACITY + 1 {
            m.push(ev(i as u32, 0, [0x90, 1, 1]));
        }
        assert_eq!(m.events().len(), LIVE_MIDI_CAPACITY, "still full at its capacity");
    }

    #[test]
    fn full_drops_and_counts_without_growing() {
        let mut m = LiveMidi::new();
        let before = m.storage();
        for i in 0..LIVE_MIDI_CAPACITY + 5 {
            m.push(ev(i as u32, 0, [0x90, 60, 100]));
        }
        assert_eq!(m.events().len(), LIVE_MIDI_CAPACITY);
        assert_eq!(m.dropped, 5);
        m.sort();
        m.clear();
        assert_eq!(m.storage(), before, "never reallocated");
    }

    #[test]
    fn filter_by_port_and_channel() {
        let mut m = LiveMidi::new();
        m.ports[0] = port_id("Keys");
        m.ports[1] = port_id("Focusrite MIDI");
        let keys_ch2 = ev(0, 0, [0x91, 60, 100]);
        let din_ch1 = ev(0, 1, [0x90, 60, 100]);
        let all = MidiFilter::new(None, 0);
        assert!(all.matches(&m, &keys_ch2) && all.matches(&m, &din_ch1));
        let keys = MidiFilter::new(Some("Keys"), 0);
        assert!(keys.matches(&m, &keys_ch2) && !keys.matches(&m, &din_ch1));
        let ch2 = MidiFilter::new(None, 2);
        assert!(ch2.matches(&m, &keys_ch2) && !ch2.matches(&m, &din_ch1));
        // An unknown port name matches nothing; a closed slot (0) is never a port.
        assert!(!MidiFilter::new(Some("Gone"), 0).matches(&m, &keys_ch2));
        assert_ne!(port_id(""), 0);
    }

    #[test]
    fn held_notes_release_once() {
        let mut h = HeldNotes::default();
        h.track(0, [0x90, 60, 100]);
        h.track(0, [0x93, 127, 1]);
        h.track(0, [0x90, 62, 100]);
        h.track(0, [0x90, 62, 0]);
        let mut offs = Vec::new();
        h.release(|m| offs.push(m));
        assert_eq!(offs, vec![[0x80, 60, 0], [0x83, 127, 0]]);
        assert!(h.is_empty());
    }

    #[test]
    fn held_pedals_are_lifted_on_release() {
        let mut h = HeldNotes::default();
        h.track(0, [0xB2, 64, 127]);
        assert!(!h.is_empty(), "a pedal down alone still holds voices");
        h.track(0, [0xB0, 66, 64]);
        h.track(0, [0xB5, 64, 100]);
        h.track(0, [0xB5, 64, 10]);
        h.track(0, [0x91, 70, 90]);
        h.track(0, [0xB1, 123, 0]);
        let mut offs = Vec::new();
        h.release(|m| offs.push(m));
        assert_eq!(offs, vec![[0xB0, 66, 0], [0xB2, 64, 0]]);
        assert!(h.is_empty());
        h.track(0, [0xB3, 64, 127]);
        h.track(0, [0xB3, 121, 0]);
        assert!(h.is_empty(), "Reset All Controllers lifts it");
    }

    #[test]
    fn release_by_port_lets_go_of_that_ports_notes_and_pedals_only() {
        let mut h = HeldNotes::default();
        h.track(0, [0x90, 60, 100]);
        h.track(3, [0x90, 50, 100]);
        h.track(3, [0xB0, 64, 127]);
        h.track(0, [0xB1, 64, 127]);
        let mut offs = Vec::new();
        h.release_ports(1 << 3, |m| offs.push(m));
        assert_eq!(offs, vec![[0x80, 50, 0], [0xB0, 64, 0]]);
        offs.clear();
        h.release(|m| offs.push(m));
        assert_eq!(offs, vec![[0x80, 60, 0], [0xB1, 64, 0]]);
        assert!(h.is_empty());
    }
}
