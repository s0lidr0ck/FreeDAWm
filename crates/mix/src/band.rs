//! The Band in the mix engine: six musicians that play into the tracks whose input is a musician
//! (`Route::Musician(k)`).
//!
//! - [`prepare_band`] builds a [`BandUpdate`] from a session off the audio thread: the host
//!   parameters from `BandState`, the players' seeds, the feel switches, the lanes as the song,
//!   and each "auto" target picked from the name of the instrument that musician plays into.
//! - [`MixEngine::adopt_band`] takes it: the first adopt builds the [`BandRuntime`] (allocates;
//!   a realtime host hands a runtime built off the audio thread over with
//!   [`MixEngine::install_band`] first), every later one is [`BandRuntime::apply`] (no allocation).
//! - Once per block, before the strips render, the band plays into [`SlotBuf`]s, one per
//!   musician; the strips only read them ([`deliver`]). Before the lanes start and from their end
//!   the band is told the transport is stopped (silence, its notes close); a locate or a stop
//!   closes every note the band holds (all notes off) in the block it happens.
//! - Old `Arc`s and spent updates go back through [`MixEngine::take_retired_band`] and
//!   [`MixEngine::take_spent_band`]: nothing is freed on the audio thread.

use std::sync::Arc;

use soundcraft_band::band::NUM_MUSICIANS;
use soundcraft_band::lanes::{LaneSong, lanes_end_half, lanes_to_timeline};
use soundcraft_band::library::LibraryData;
pub use soundcraft_band::library::auto_target;
use soundcraft_band::midi_types::{MidiEvent, MidiSink};
use soundcraft_band::runtime::{BandRuntime, BandSettings, FeelFlags, HostTransport, Retired};
use soundcraft_band::song::SongTimeline;
use soundcraft_model::{Route, Session, TrackId};
use soundcraft_time::Samples;

use crate::band_lanes::{half_ticks, lane_song};
use crate::{MixEngine, PluginSlot, transport_at};

/// One slot per musician.
pub const BAND_SLOTS: usize = NUM_MUSICIANS;
/// Events one slot holds per block (reserved at [`MixEngine::new`]).
pub const SLOT_CAPACITY: usize = 8192;

/// One musician's events for the block: `(offset, bytes, lower)`. `lower` marks the organ's
/// lower manual (its channel byte is kept on delivery).
#[derive(Debug, Default)]
pub struct SlotBuf {
    pub events: Vec<(u32 /* offset */, [u8; 3], bool /* lower */)>,
    /// Events that did not fit this block (never grown on the audio thread).
    pub dropped: u32,
}

impl SlotBuf {
    pub fn with_capacity(n: usize) -> SlotBuf {
        SlotBuf { events: Vec::with_capacity(n), dropped: 0 }
    }

    /// Adds an event, or counts it as dropped when the reserved capacity is full. No allocation.
    pub fn push(&mut self, offset: u32, bytes: [u8; 3], lower: bool) -> bool {
        if self.events.len() < self.events.capacity() {
            self.events.push((offset, bytes, lower));
            true
        } else {
            self.dropped = self.dropped.saturating_add(1);
            false
        }
    }

    fn clear(&mut self) {
        self.events.clear();
        self.dropped = 0;
    }
}

/// What the engine's band plays: its settings, and where the lanes start and end (`None`: nothing
/// on them).
#[derive(Debug)]
pub struct BandUpdate {
    pub settings: BandSettings,
    pub song_end: Option<Samples>,
    pub start: Option<Samples>,
}

/// The lanes' song the last update was built from, so an update that leaves them unchanged keeps
/// the same timeline (the band swaps its song only when it changed).
#[derive(Debug, Default)]
pub struct BandSongCache {
    last: Option<(LaneSong, Option<Arc<SongTimeline>>)>,
}

fn track_musician(r: &Route) -> Option<usize> {
    match r {
        Route::Musician(k) => Some(usize::from(*k)),
        _ => None,
    }
}

fn half_to_samples(s: &Session, half: i32) -> Samples {
    s.tempo.tick_to_samples(i64::from(half).saturating_mul(half_ticks(s)), s.sample_rate)
}

/// The band's update for `s`, or `None` when no track's input is a musician. Off the audio thread
/// (it allocates). `instrument_name` names the instrument plugin on a track (its live instance's
/// name), for the "auto" targets.
pub fn prepare_band(s: &Session, lib: &Arc<LibraryData>, instrument_name: &dyn Fn(TrackId) -> Option<String>) -> Option<Box<BandUpdate>> {
    prepare_band_cached(s, lib, instrument_name, &mut BandSongCache::default())
}

/// [`prepare_band`], keeping the last timeline when the lanes are unchanged.
pub fn prepare_band_cached(
    s: &Session,
    lib: &Arc<LibraryData>,
    instrument_name: &dyn Fn(TrackId) -> Option<String>,
    cache: &mut BandSongCache,
) -> Option<Box<BandUpdate>> {
    if !s.tracks.iter().any(|t| track_musician(&t.mixer.input).is_some()) {
        return None;
    }
    let b = &s.band;
    let mut st = BandSettings::processor_default(Arc::clone(lib));
    st.set_param("key", f32::from(b.key));
    st.set_param("scale", f32::from(b.scale));
    st.set_param("take", f32::from(b.take));
    for (k, m) in b.slots.iter().enumerate() {
        let mut set = |field: &str, v: f32| {
            let known = st.set_param(&format!("m{}_{field}", k + 1), v);
            debug_assert!(known, "m{}_{field} is a host parameter", k + 1);
        };
        set("instrument", f32::from(m.instrument));
        set("energy", f32::from(m.energy));
        set("swing", f32::from(m.swing));
        set("humanize", f32::from(m.humanize));
        set("dynamics", f32::from(m.dynamics));
        set("fills", f32::from(m.fills));
        set("respect", if m.respect { 1.0 } else { 0.0 });
        set("octave", f32::from(m.octave));
        set("length", f32::from(m.length));
        set("velocity", f32::from(m.velocity));
        // Internal channels are k+1 (they order the interplay); delivery puts them on channel 1.
        set("channel", (k + 1) as f32);
        set("swaminst", f32::from(m.swaminst));
        set("voice", f32::from(m.voice));
        set("speed", f32::from(m.speed));
        set("enabled", if m.enabled { 1.0 } else { 0.0 });
        set("mute", if m.mute { 1.0 } else { 0.0 });
        for (field, a) in ["art_long", "art_short", "art_pluck", "art_trem"].into_iter().zip(m.art) {
            set(field, f32::from(a));
        }
        set("ample", f32::from(m.ample));
        set("groove", f32::from(m.groove));
        set("amplechords", f32::from(m.amplechords));
        set("pluckchords", f32::from(m.pluckchords));
        let auto = || {
            let name = s.tracks.iter().find(|t| track_musician(&t.mixer.input) == Some(k)).and_then(|t| instrument_name(t.id)).unwrap_or_default();
            auto_target(lib, &name)
        };
        let target = if m.target.eq_ignore_ascii_case("auto") {
            auto()
        } else {
            usize::try_from(lib.target_slot_for_name(&m.target)).unwrap_or_else(|_| auto())
        };
        set("target", target as f32);
        let ks = m.ksoctave.map(i32::from).or_else(|| lib.target(target as i32).map(|t| t.ks_octave_default)).unwrap_or(0);
        set("ksoctave", ks as f32);
    }
    // Players last: an instrument change re-picks them.
    for (k, m) in b.slots.iter().enumerate() {
        let inst = st.param(&format!("m{}_instrument", k + 1)).map_or(0, |v| v.round() as i32);
        let seed = lib.seed_for_name(&m.player);
        let seed = if seed > 0 { seed } else { i32::from(m.player_seed) };
        if seed > 0 && lib.plays(inst, seed) {
            if let Some(x) = st.seeds.get_mut(k) {
                *x = seed;
            }
        } else {
            // No player, or one the Library lacks or who doesn't play the instrument (a stale
            // name or seed after a Library change): MS `setPlayer`, the instrument's first.
            st.set_player(k, &m.player);
        }
    }
    let f = &b.feel;
    st.feel = FeelFlags {
        mod_wheel: false,
        bass_lock: f.bass_lock,
        fills_yield: f.fills_yield,
        leave_space: f.leave_space,
        dynamics: f.dynamics,
        natural: f.natural,
        tension: f.tension,
    };
    let (song, start, song_end) = match lane_song(s) {
        Some(lanes) => {
            let first = lanes.blocks.iter().map(|x| x.start_half).chain(lanes.chords.iter().map(|x| x.start_half)).min();
            let end = lanes_end_half(&lanes);
            let song = match &cache.last {
                Some((l, song)) if *l == lanes => song.clone(),
                _ => lanes_to_timeline(&lanes),
            };
            cache.last = Some((lanes, song.clone()));
            let bounds = (end > 0).then(|| (first.map(|h| half_to_samples(s, h.max(0))), half_to_samples(s, end)));
            (song, bounds.and_then(|(a, _)| a), bounds.map(|(_, e)| e))
        }
        None => {
            cache.last = None;
            (None, None, None)
        }
    };
    st.song = song;
    Some(Box::new(BandUpdate { settings: st, song_end, start }))
}

/// Where the lanes start (their first block or chord), or `None` with nothing on them. Allocates.
fn lanes_start(s: &Session) -> Option<Samples> {
    let lanes = lane_song(s)?;
    let first = lanes.blocks.iter().map(|x| x.start_half).chain(lanes.chords.iter().map(|x| x.start_half)).min()?;
    Some(half_to_samples(s, first.max(0)))
}

/// The band's play head at `pos`: the session's tempo there and its meter at tick 0 (the lanes'
/// grid), its position in quarters as `ppq`. Before the lanes start, at or past their end (and
/// with nothing on them) it reports `playing: false`, so the band is silent there and closes its
/// notes. Allocates (it reads the lanes); the engine uses the bounds of its update instead.
pub fn band_transport(s: &Session, pos: Samples, playing: bool) -> HostTransport {
    band_transport_within(s, pos, playing, lanes_start(s), crate::band_song_end(s))
}

/// [`band_transport`] with the lanes' bounds given (a [`BandUpdate`]'s `start` and `song_end`).
/// No allocation.
pub fn band_transport_within(s: &Session, pos: Samples, playing: bool, start: Option<Samples>, end: Option<Samples>) -> HostTransport {
    let t = transport_at(s, pos, playing, false);
    let inside = start.is_some_and(|a| pos >= a) && end.is_some_and(|e| pos < e);
    // The band follows the session's meter at tick 0, as the lanes' half bars do: a later meter
    // change would make it read the timeline with other halves than the lanes show.
    let m = s.tempo.meter_at_tick(0);
    HostTransport {
        playing: playing && inside,
        bpm: t.tempo,
        ppq: t.quarters,
        num: i32::try_from(m.numerator).unwrap_or(4),
        den: i32::try_from(m.denominator).unwrap_or(4),
    }
}

/// A slot's events into an instrument: every event but the organ's lower manual goes on channel 1.
pub(crate) fn deliver(slot: &SlotBuf, inst: &mut PluginSlot) {
    for &(offset, bytes, lower) in &slot.events {
        let mut b = bytes;
        if !lower && b[0] < 0xF0 {
            b[0] &= 0xF0;
        }
        inst.plugin.midi_raw(offset as usize, b);
    }
}

/// The band's events into the slots, by musician, `base` samples into the block.
struct SlotSink<'a> {
    slots: &'a mut [SlotBuf; BAND_SLOTS],
    base: u32,
}

impl MidiSink for SlotSink<'_> {
    fn event(&mut self, e: &MidiEvent) {
        if e.rendered {
            return;
        }
        let k = match usize::try_from(e.musician) {
            Ok(k) => k,
            // Not a musician's own: by its channel (k+1), unless it is the lower manual's.
            Err(_) if !e.lower => usize::from(e.status & 0x0F).wrapping_sub(1),
            Err(_) => return,
        };
        let Some(slot) = self.slots.get_mut(k) else { return };
        let offset = self.base.saturating_add(u32::try_from(e.offset).unwrap_or(0));
        slot.push(offset, [e.status, e.d1, e.d2], e.lower);
    }
}

pub(crate) fn new_slots() -> [SlotBuf; BAND_SLOTS] {
    std::array::from_fn(|_| SlotBuf::with_capacity(SLOT_CAPACITY))
}

/// A runtime for `u`, built off the audio thread (see [`MixEngine::install_band`]).
pub fn new_band_runtime(sample_rate: f32, u: &BandUpdate) -> Box<BandRuntime> {
    Box::new(BandRuntime::new(f64::from(sample_rate), u.settings.clone()))
}

impl MixEngine {
    /// Takes new band settings. The first adopt builds the runtime (allocates: a realtime host
    /// installs one built off the audio thread first, [`MixEngine::install_band`]); later ones
    /// apply without allocating. The update this one replaces waits for
    /// [`MixEngine::take_spent_band`], the `Arc`s it displaces for [`MixEngine::take_retired_band`].
    pub fn adopt_band(&mut self, u: Box<BandUpdate>) {
        match self.band.as_deref_mut() {
            Some(rt) => rt.apply(u.settings.clone(), &mut self.band_retired),
            None => self.band = Some(new_band_runtime(self.sample_rate, &u)),
        }
        if let Some(old) = self.band_update.replace(u) {
            self.park_spent(old);
        }
    }

    /// Installs a runtime built off the audio thread ([`new_band_runtime`]). One already there
    /// stays; the given one is handed back, to be dropped elsewhere.
    #[must_use = "a refused runtime must be dropped off the audio thread"]
    pub fn install_band(&mut self, rt: Box<BandRuntime>) -> Option<Box<BandRuntime>> {
        if self.band.is_some() {
            return Some(rt);
        }
        self.band = Some(rt);
        None
    }

    /// The `Arc`s the band let go of, to drop off the audio thread. No allocation.
    pub fn take_retired_band(&mut self) -> Retired {
        std::mem::take(&mut self.band_retired)
    }

    /// A band update that was replaced, to drop off the audio thread (one per call).
    pub fn take_spent_band(&mut self) -> Option<Box<BandUpdate>> {
        self.band_spent.iter_mut().find_map(Option::take)
    }

    /// The engine has a band runtime.
    pub fn has_band(&self) -> bool {
        self.band.is_some()
    }

    /// The band's events of the last block, per musician.
    pub fn band_out(&self) -> &[SlotBuf; BAND_SLOTS] {
        &self.band_out
    }

    /// Takes the band out (a new engine for a new sample rate gets a runtime built for its rate):
    /// the runtime is returned, and the update it played waits for
    /// [`MixEngine::take_spent_band`]. Nothing is freed or prepared here.
    #[must_use = "the runtime must be dropped off the audio thread"]
    pub fn take_band(&mut self) -> Option<Box<BandRuntime>> {
        if let Some(old) = self.band_update.take() {
            self.park_spent(old);
        }
        self.band_playing = false;
        self.band.take()
    }

    fn park_spent(&mut self, old: Box<BandUpdate>) {
        match self.band_spent.iter_mut().find(|x| x.is_none()) {
            Some(slot) => *slot = Some(old),
            // Never expected (the host takes them after every adopt): never free here.
            None => {
                let _ = Box::leak(old);
            }
        }
    }

    /// The name of the instrument plugin on track `t` (its live instance), if it has one.
    pub fn instrument_name(&self, t: TrackId) -> Option<String> {
        let inst = self.strips.get(&t)?.instrument.as_ref()?;
        Some(inst.plugin.info().name.to_string())
    }

    /// Test support: every slot's events capacity (a forced small one exercises overflow).
    /// Allocates.
    #[doc(hidden)]
    pub fn set_band_slot_capacity(&mut self, n: usize) {
        for b in &mut self.band_out {
            *b = SlotBuf::with_capacity(n);
        }
    }

    /// Test support: (address, capacity) of the slot buffers and of every buffer the runtime owns.
    #[doc(hidden)]
    pub fn band_buffers(&self) -> Vec<(usize, usize)> {
        let mut v: Vec<(usize, usize)> = self.band_out.iter().map(|b| (b.events.as_ptr() as usize, b.events.capacity())).collect();
        if let Some(rt) = &self.band {
            v.extend(rt.buffer_addresses());
        }
        v
    }

    /// The band's block, before the strips render. No allocation.
    pub(crate) fn render_band(&mut self, s: &Session, pos: Samples, frames: usize, seek: bool) {
        for b in &mut self.band_out {
            b.clear();
        }
        let Some(rt) = self.band.as_deref_mut() else { return };
        let mut sink = SlotSink { slots: &mut self.band_out, base: 0 };
        if !s.tracks.iter().any(|t| !t.inactive && track_musician(&t.mixer.input).is_some()) {
            // Nobody listens: the band rests (what it held closes).
            if std::mem::replace(&mut self.band_playing, false) {
                rt.flush(&mut sink);
            }
            return;
        }
        let (start, end) = self.band_update.as_ref().map_or((None, None), |u| (u.start, u.song_end));
        let playing = !self.monitor_only;
        // A locate (or a loop wrap): all notes off, then MS's own reset rule on the jump.
        if seek && self.band_playing {
            rt.flush(&mut sink);
            self.band_playing = false;
        }
        // The block, cut where the lanes start or end so the band starts and stops on the sample.
        let block_end = pos.saturating_add(frames as i64);
        let mut cuts = [block_end; 3];
        let mut n = 0;
        for b in [start, end].into_iter().flatten() {
            if b > pos && b < block_end {
                if let Some(c) = cuts.get_mut(n) {
                    *c = b;
                }
                n += 1;
            }
        }
        if let Some(c) = cuts.get_mut(..n) {
            c.sort_unstable();
        }
        let mut at = pos;
        for cut in cuts {
            if cut <= at {
                continue;
            }
            let t = band_transport_within(s, at, playing, start, end);
            sink.base = u32::try_from(at - pos).unwrap_or(0);
            if self.band_playing && !t.playing {
                rt.flush(&mut sink); // a stop, or the lanes' end
            }
            self.band_playing = t.playing;
            rt.process_block(&t, usize::try_from(cut - at).unwrap_or(0), &mut sink);
            at = cut;
        }
    }
}
