//! Port of `MidiTypes.h`.

/// One message the engine sent. `ppq` is the engine's float(currentPpq) exactly as Amorph
/// logged it; `keyswitch` is the +0x100 log flag (sendKeyswitch).
#[derive(Clone, Copy, Debug)]
pub struct MidiEvent {
    /// Sample offset inside the block (live); 0 when rendering.
    pub offset: i32,
    pub ppq: f32,
    pub status: u8,
    pub d1: u8,
    pub d2: u8,
    pub keyswitch: bool,
    /// True when produced in render mode (log only, not midiOut).
    pub rendered: bool,
    /// The Hammond's lower manual (its own channel, not the musician's): who played it. Not compared.
    pub lower: bool,
    pub musician: i8,
}

impl Default for MidiEvent {
    fn default() -> Self {
        Self { offset: 0, ppq: 0.0, status: 0, d1: 0, d2: 0, keyswitch: false, rendered: false, lower: false, musician: -1 }
    }
}

impl PartialEq for MidiEvent {
    /// Compares exactly the C++ fields (`lower` and `musician` are not compared).
    fn eq(&self, o: &Self) -> bool {
        self.offset == o.offset
            && self.ppq == o.ppq
            && self.status == o.status
            && self.d1 == o.d1
            && self.d2 == o.d2
            && self.keyswitch == o.keyswitch
            && self.rendered == o.rendered
    }
}

pub trait MidiSink {
    fn event(&mut self, e: &MidiEvent);
    /// Render start/end markers (log codes 0x180 / 0x181). `bars` is 0 for the end marker.
    fn render_marker(&mut self, _start: bool, _bars: i32) {}
    /// While rendering offline: what a musician decided at ppq (1 fill, 2 pickup, 4 push, 8 varied bar).
    fn annotate(&mut self, _musician: i32, _kind: i32, _ppq: f64) {}
}

/// Unbounded: the port must never drop events (the Cmajor reference caps at 32 per advance).
#[derive(Clone, Debug, Default)]
pub struct VecSink {
    pub events: Vec<MidiEvent>,
    pub markers: Vec<(bool, i32)>,
}

impl VecSink {
    pub fn with_capacity(n: usize) -> Self {
        Self { events: Vec::with_capacity(n), markers: Vec::new() }
    }
}

impl MidiSink for VecSink {
    fn event(&mut self, e: &MidiEvent) {
        self.events.push(*e);
    }
    fn render_marker(&mut self, start: bool, bars: i32) {
        self.markers.push((start, bars));
    }
}
