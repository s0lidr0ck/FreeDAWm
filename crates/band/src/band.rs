//! Port of `Band.h`: the port of DSP.txt `graph Band`, six Musicians sharing one song.
//!
//! Matches the generated graph's behaviour exactly, which the A/B depends on:
//!   * shared endpoints fan out to m1..m6 in node order;
//!   * process() runs frame by frame, stepping m1..m6 on each frame (not a whole block per node);
//!   * MIDI a musician sends while handling a parameter or transport change lands at frame 0 of the
//!     next block, in the order it was sent, ahead of that block's frame events.
//!
//! Rust differences (form only): the Library, the song and the interplay record are passed to the musicians as a
//! `musician::Ctx`; endpoints are routed by index ([`Band::endpoint_index`] once, [`Band::set_param_at`] on the
//! audio thread). Everything is allocated in [`Band::new`]; nothing on the audio path allocates.

use std::sync::Arc;

use crate::library::LibraryData;
use crate::live::{BandShared, PerformanceState};
use crate::meter::{Meter, meter_from_host};
use crate::midi_types::{MidiSink, VecSink};
use crate::musician::{Capture, CaptureEvent, Ctx, Musician, MusicianDefaults, generic_target, session_feel};
use crate::song::SongTimeline;

pub const NUM_MUSICIANS: usize = 6;

/// `pending` capacity: emissions from parameter / transport handlers between two blocks.
const PENDING_CAPACITY: usize = 4096;

#[allow(clippy::too_many_arguments)] // DSP.txt's argument order, one row per musician
const fn md(
    channel: i32,
    on: bool,
    instrument: i32,
    player: i32,
    idea: i32,
    energy: i32,
    swing: f32,
    humanize: f32,
    dynamics: f32,
    fills: f32,
    respect: bool,
    octave: i32,
    length: f32,
    velocity: i32,
    target: i32,
    ks: i32,
    swam_inst: i32,
    voice: i32,
    speed: i32,
    art_map: i32,
) -> MusicianDefaults {
    MusicianDefaults {
        channel,
        on,
        instrument,
        player,
        idea,
        energy,
        swing,
        humanize,
        dynamics,
        fills,
        respect,
        octave,
        length,
        velocity,
        target,
        ks,
        swam_inst,
        voice,
        speed,
        art_map,
    }
}

/// The graph's node specialisations (DSP.txt lines 2672-2677), in argument order.
//                            ch  on    inst pl idea en swing   hum     dyn     fills  resp  oct  len    vel tgt ks si vo sp art
static FACTORY_DEFAULTS: [MusicianDefaults; NUM_MUSICIANS] = [
    md(0, true, 0, 1, 3, 1, 50.0, 100.0, 100.0, 42.0, true, -1, 1.00, 71, 4, 0, 0, 0, 0, 12816),
    md(1, true, 5, 1, 2, 1, 100.0, 100.0, 100.0, 50.0, true, -2, 1.00, 78, 1, 0, 0, 0, 0, 12816),
    md(2, true, 2, 7, 2, 1, 50.0, 100.0, 100.0, 0.0, true, 0, 1.00, 41, 2, 0, 0, 0, 0, 12816),
    md(3, true, 7, 1, 0, 4, 0.0, 100.0, 100.0, 100.0, true, 2, 1.00, 51, 10, -1, 0, 0, 1, 12816),
    md(4, true, 9, 1, 2, 4, 50.0, 100.0, 100.0, 100.0, true, 0, 0.10, 77, 10, 0, 0, 0, 0, 12816),
    md(5, false, 0, 0, 0, 3, 50.0, 50.0, 50.0, 40.0, false, 0, 0.85, 96, 0, 0, 0, 0, 1, 12816),
];

// The `connection` block (DSP.txt 2680-2830): endpoint -> musician paramN (index N-1). Routes 0..14 are shared
// (fan out to all six), then 26 per musician, m1 first.
const SHARED_ROUTES: [(&str, usize); 14] = [
    ("key", 2),
    ("scale", 3),
    ("progression", 13),
    ("bars", 14),
    ("chord1", 15),
    ("chord2", 16),
    ("chord3", 17),
    ("chord4", 18),
    ("chord5", 19),
    ("chord6", 20),
    ("chord7", 21),
    ("chord8", 22),
    ("take", 28),
    ("chordbars", 34),
];
const PER_ROUTES: [(&str, usize); 26] = [
    ("instrument", 1),
    ("player", 11),
    ("idea", 10),
    ("energy", 12),
    ("swing", 23),
    ("humanize", 24),
    ("dynamics", 25),
    ("fills", 26),
    ("respect", 27),
    ("octave", 4),
    ("length", 6),
    ("velocity", 7),
    ("channel", 8),
    ("target", 29),
    ("ksoctave", 30),
    ("swaminst", 31),
    ("voice", 32),
    ("speed", 5),
    ("artmap", 35),
    ("on", 9),
    ("mute", 36),
    ("render", 33),
    ("ample", 37),
    ("groove", 38),
    ("amplechords", 39),
    ("pluckchords", 40),
];
/// Every route: 14 shared + 6 x 26.
pub const NUM_ROUTES: usize = SHARED_ROUTES.len() + NUM_MUSICIANS * PER_ROUTES.len();

/// Route `i`: (musician, paramIndex); musician None = all six.
fn route(i: usize) -> Option<(Option<usize>, usize)> {
    if let Some(&(_, n)) = SHARED_ROUTES.get(i) {
        return Some((None, n - 1));
    }
    let j = i - SHARED_ROUTES.len();
    let k = j / PER_ROUTES.len();
    if k >= NUM_MUSICIANS {
        return None;
    }
    PER_ROUTES.get(j % PER_ROUTES.len()).map(|&(_, n)| (Some(k), n - 1))
}

pub struct Band {
    lib: Arc<LibraryData>,
    song: Option<Arc<SongTimeline>>,
    perf: PerformanceState,
    last_playing: bool,
    last_ppq: f64,
    /// interplay: what the musicians tell each other
    shared: BandShared,
    /// kept takes: everything the band sends, for the message thread (a no-op here)
    capture_ring: Capture,
    sample_rate: f64,
    musicians: Vec<Musician>,
    /// emissions from parameter / transport handlers, delivered at frame 0 of the next block
    pending: VecSink,
    /// A groove audition: only musician `solo` plays (-1 = everyone).
    pub solo: i32,
}

impl Band {
    /// Allocates everything the band will ever use (the musicians, `pending` with room for 4096 events) and builds
    /// the shared tables (the Session feel, the built-in Generic, the parameter tables) so no audio call does.
    pub fn new(lib: Arc<LibraryData>) -> Band {
        let _ = (session_feel(), generic_target(), crate::params::host_params(), crate::params::packed_endpoints());
        let mut b = Band {
            lib,
            song: None,
            perf: PerformanceState::default(),
            last_playing: false,
            last_ppq: 0.0,
            shared: BandShared::default(),
            capture_ring: Capture,
            sample_rate: 48000.0,
            musicians: Vec::with_capacity(NUM_MUSICIANS),
            pending: VecSink { events: Vec::with_capacity(PENDING_CAPACITY), markers: Vec::with_capacity(16) },
            solo: -1,
        };
        for d in &FACTORY_DEFAULTS {
            b.musicians.push(Musician::new(*d));
        }
        b.rebuild();
        b
    }

    /// The graph's node specialisations (DSP.txt lines 2672-2677), in argument order.
    pub fn factory_defaults() -> &'static [MusicianDefaults; NUM_MUSICIANS] {
        &FACTORY_DEFAULTS
    }

    /// Starts from the graph's initial state again (as the Cmajor initialise does). The caller re-sends its
    /// parameters afterwards; the runtime does that with its force-push.
    pub fn prepare(&mut self, sr: f64) {
        self.sample_rate = sr;
        self.rebuild();
    }

    /// The route index of an Amorph Band endpoint name (key, chord3, m4_target, ...); None for an unknown name.
    pub fn endpoint_index(name: &str) -> Option<usize> {
        if let Some(i) = SHARED_ROUTES.iter().position(|&(n, _)| n == name) {
            return Some(i);
        }
        let b = name.as_bytes();
        if b.len() < 4 || b[0] != b'm' || b[2] != b'_' || !(b'1'..=b'6').contains(&b[1]) {
            return None;
        }
        let k = usize::from(b[1] - b'1');
        let field = name.get(3..)?;
        let j = PER_ROUTES.iter().position(|&(n, _)| n == field)?;
        Some(SHARED_ROUTES.len() + k * PER_ROUTES.len() + j)
    }

    /// `setParam (endpoint, v)` by route index (from [`Band::endpoint_index`]). Unknown indices are ignored.
    pub fn set_param_at(&mut self, i: usize, v: f32) {
        let Some((who, param)) = route(i) else { return };
        let mut cx = Ctx { lib: &self.lib, song: self.song.as_deref(), perf: &self.perf, shared: &mut self.shared };
        match who {
            None => {
                for m in &mut self.musicians {
                    m.set_parameter(param, v, &mut cx, &mut self.pending);
                }
            }
            Some(k) => {
                if let Some(m) = self.musicians.get_mut(k) {
                    m.set_parameter(param, v, &mut cx, &mut self.pending);
                }
            }
        }
    }

    pub fn set_host_transport(&mut self, playing: bool, bpm: f64, ppq: f64, num: i32, den: i32) {
        let broke = playing != self.last_playing || (playing && ppq < self.last_ppq - 1.0e-6);
        if broke {
            self.shared.clear(); // a stop, a start or a jump back
        }
        // Kept takes: where the band is (song position), and any break in it.
        let perf = &self.perf;
        let song_pos = ppq + if perf.jump_land >= 0.0 && ppq >= perf.jump_land - 1.0e-9 { perf.jump_new_offset } else { perf.jump_offset };
        // A break carries the meter the band plays (d1, d2 = 1). A change while playing is a break of its own (d2 = 2),
        // posted by the musicians where they switch (Musician::switchMeterAt); the recorder follows.
        let (hm, _unsupported) = meter_from_host(num, den);
        let m = match self.song.as_deref() {
            Some(s) if s.meter_override >= 0 => Meter::from_index(s.meter_override),
            _ => hm,
        };
        if broke {
            self.capture_ring.push(CaptureEvent { ppq: song_pos, musician: -1, d1: m as u8, d2: 1, ..CaptureEvent::default() });
        }
        if playing {
            self.capture_ring.push(CaptureEvent { ppq: song_pos, musician: -2, ..CaptureEvent::default() });
        }
        self.last_playing = playing;
        self.last_ppq = ppq;
        let mut cx = Ctx { lib: &self.lib, song: self.song.as_deref(), perf: &self.perf, shared: &mut self.shared };
        for mu in &mut self.musicians {
            mu.set_host_transport(playing, bpm, ppq, num, den, &mut cx, &mut self.pending);
        }
    }

    pub fn midi_in(&mut self, data: &[u8]) {
        let mut cx = Ctx { lib: &self.lib, song: self.song.as_deref(), perf: &self.perf, shared: &mut self.shared };
        for m in &mut self.musicians {
            m.midi_in(data, &mut cx, &mut self.pending);
        }
    }

    /// Swaps the song in (None = the Amorph song endpoints) and returns the old one (the caller retires it).
    pub fn set_song(&mut self, t: Option<Arc<SongTimeline>>) -> Option<Arc<SongTimeline>> {
        let changed = match (&self.song, &t) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        };
        let had_song = self.song.is_some();
        let old = std::mem::replace(&mut self.song, t);
        let mut cx = Ctx { lib: &self.lib, song: self.song.as_deref(), perf: &self.perf, shared: &mut self.shared };
        for m in &mut self.musicians {
            m.set_song(changed, had_song, &mut cx, &mut self.pending);
        }
        old
    }

    pub fn set_performance(&mut self, p: &PerformanceState) {
        self.perf = *p;
        let mut cx = Ctx { lib: &self.lib, song: self.song.as_deref(), perf: &self.perf, shared: &mut self.shared };
        for m in &mut self.musicians {
            m.set_performance(p, &mut cx, &mut self.pending);
        }
    }

    pub fn current_song(&self) -> Option<&Arc<SongTimeline>> {
        self.song.as_ref()
    }
    pub(crate) fn song_option(&self) -> &Option<Arc<SongTimeline>> {
        &self.song
    }
    /// The audio thread's view (no refcount traffic).
    pub fn song_now(&self) -> Option<&SongTimeline> {
        self.song.as_deref()
    }

    pub fn set_persona_seed(&mut self, k: usize, seed: i32) {
        let mut cx = Ctx { lib: &self.lib, song: self.song.as_deref(), perf: &self.perf, shared: &mut self.shared };
        if let Some(m) = self.musicians.get_mut(k) {
            m.set_persona_seed(seed, &mut cx, &mut self.pending);
        }
    }

    pub fn process(&mut self, frames: usize, out: &mut dyn MidiSink) {
        self.shared.drum_known = false; // Natural feel: who leads the band's time
        self.shared.bass_known = false;
        for m in &self.musicians {
            let mut map = [0.0f32; 16];
            let lead = m.groove_lead(&mut map, &self.lib);
            if lead == 5 && !self.shared.drum_known {
                self.shared.drum_groove = map;
                self.shared.drum_known = true;
            } else if lead == 2 && !self.shared.bass_known {
                self.shared.bass_groove = map;
                self.shared.bass_known = true;
            }
        }
        for e in &self.pending.events {
            out.event(e);
        }
        for &(start, bars) in &self.pending.markers {
            out.render_marker(start, bars);
        }
        self.pending.events.clear();
        self.pending.markers.clear();
        let mut cx = Ctx { lib: &self.lib, song: self.song.as_deref(), perf: &self.perf, shared: &mut self.shared };
        if self.perf.bass_lock {
            // the drummer speaks first, so the bass hears this step's kick
            for f in 0..frames {
                for m in &mut self.musicians {
                    if m.is_drums() {
                        m.frame(f, &mut cx, out);
                    }
                }
                for m in &mut self.musicians {
                    if !m.is_drums() {
                        m.frame(f, &mut cx, out);
                    }
                }
            }
            return;
        }
        if self.solo >= 0
            && let Some(m) = usize::try_from(self.solo).ok().and_then(|k| self.musicians.get_mut(k))
        {
            // a groove audition: that musician alone
            for f in 0..frames {
                m.frame(f, &mut cx, out);
            }
            return;
        }
        for f in 0..frames {
            for m in &mut self.musicians {
                m.frame(f, &mut cx, out);
            }
        }
    }

    /// A groove audition: only musician k plays (-1 = everyone), at Groove g (-1 = its slider).
    pub fn set_audition(&mut self, k: i32, g: f32) {
        self.solo = k;
        for (i, m) in self.musicians.iter_mut().enumerate() {
            m.set_groove_override(if i32::try_from(i) == Ok(k) { g } else { -1.0 });
        }
    }

    /// Swap the Library between blocks: every musician closes its notes first (into `flush_to`). Returns the old one
    /// (the caller retires it).
    pub fn set_library(&mut self, lib: Arc<LibraryData>, flush_to: &mut dyn MidiSink) -> Arc<LibraryData> {
        self.flush(flush_to);
        std::mem::replace(&mut self.lib, lib)
    }

    pub fn flush(&mut self, out: &mut dyn MidiSink) {
        let mut cx = Ctx { lib: &self.lib, song: self.song.as_deref(), perf: &self.perf, shared: &mut self.shared };
        for m in &mut self.musicians {
            m.flush(&mut cx, out);
        }
    }

    pub fn library(&self) -> &Arc<LibraryData> {
        &self.lib
    }

    // Note: the graph's endpoint `init:` values are host metadata; the Cmajor runtime does not send
    // them at initialise, so neither does this. The runtime pushes every parameter on its first block.
    fn rebuild(&mut self) {
        let mut cx = Ctx { lib: &self.lib, song: self.song.as_deref(), perf: &self.perf, shared: &mut self.shared };
        for (k, m) in self.musicians.iter_mut().enumerate() {
            *m = Musician::new(FACTORY_DEFAULTS.get(k).copied().unwrap_or_default());
            m.index = k as i32;
            m.prepare(self.sample_rate);
            // setIdleSink (&pending): `pending` is passed as `idle` to every out-of-frame call.
            m.set_song(cx.song.is_some(), false, &mut cx, &mut self.pending); // the song survives a re-prepare
            m.set_performance(&self.perf, &mut cx, &mut self.pending);
            // setShared (&shared): `cx.shared`.
            m.set_capture(Some(Capture));
        }
        self.pending.events.clear();
        self.pending.markers.clear();
    }

    /// Test support: (address, capacity) of every buffer the band owns, which must never move on the audio thread.
    #[doc(hidden)]
    pub fn buffer_addresses(&self) -> Vec<(usize, usize)> {
        let mut v = vec![
            (self.pending.events.as_ptr() as usize, self.pending.events.capacity()),
            (self.pending.markers.as_ptr() as usize, self.pending.markers.capacity()),
            (self.musicians.as_ptr() as usize, self.musicians.capacity()),
        ];
        for m in &self.musicians {
            for a in m.buffer_addresses() {
                v.push((a, 0));
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_resolve_like_band_h() {
        assert_eq!(Band::endpoint_index("key"), Some(0));
        assert_eq!(route(0), Some((None, 1)));
        let i = Band::endpoint_index("m4_target");
        assert_eq!(i.and_then(route), Some((Some(3), 28)));
        assert_eq!(Band::endpoint_index("m6_pluckchords").and_then(route), Some((Some(5), 39)));
        assert_eq!(Band::endpoint_index("m7_target"), None);
        assert_eq!(Band::endpoint_index("m1_enabled"), None);
        assert_eq!(route(NUM_ROUTES), None);
        // Every packed endpoint the processor sends has a route.
        for e in crate::params::packed_endpoints() {
            assert!(Band::endpoint_index(&e.name).is_some(), "{} has no route", e.name);
        }
    }

    #[test]
    fn factory_defaults_are_dsp_txt() {
        let d = Band::factory_defaults();
        assert_eq!((d[2].instrument, d[2].player, d[2].velocity, d[2].target), (2, 7, 41, 2));
        assert!(!d[5].on);
        assert_eq!(d[3].ks, -1);
    }
}
