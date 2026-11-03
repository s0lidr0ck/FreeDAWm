//! `BandRuntime`: `MidiStudioProcessor::processBlock`'s audio path (`PluginProcessor.cpp` 145-357) around a [`Band`],
//! and [`BandSettings`], the processor state that drives it (host parameters, players, feel switches, song, Library).
//!
//! The references (`tools/band-ref`) are rendered by an offline `MidiStudioProcessor (true)`, so this ports exactly
//! the processor's per-block work: parameter packing (only what changed), the persona seeds, the song and Library
//! swaps, the Performer's feel flags (Live off), carry-over note-offs, the transport and `Band::process`.
//!
//! Audio-thread rules: [`BandRuntime::apply`] and [`BandRuntime::process_block`] never allocate, lock or free. Old
//! `Arc`s leave through [`Retired`] (the caller frees them off the audio thread).

use std::sync::{Arc, OnceLock};

use crate::band::{Band, NUM_MUSICIANS};
use crate::library::{K_INSTRUMENTS, K_PLAYER_SLOTS, LibraryData};
use crate::live::Performer;
use crate::meter::{Meter, info, meter_from_host};
use crate::midi_types::{MidiEvent, MidiSink, VecSink};
use crate::params::{HOST_PARAMS, HostParamKind, PACKED, host_param_index, host_params, pack_into, packed_endpoints, round_to_int};
use crate::song::{LegacySong, SongTimeline, build_timeline};

/// The host's play head for one block (`AudioPlayHead::PositionInfo`: playing, bpm, ppq, time signature).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HostTransport {
    pub playing: bool,
    pub bpm: f64,
    pub ppq: f64,
    pub num: i32,
    pub den: i32,
}

impl Default for HostTransport {
    fn default() -> Self {
        Self { playing: false, bpm: 120.0, ppq: 0.0, num: 4, den: 4 }
    }
}

/// `MidiStudioProcessor::Feel` switches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeelFlags {
    pub mod_wheel: bool,
    pub bass_lock: bool,
    pub fills_yield: bool,
    pub leave_space: bool,
    pub dynamics: bool,
    pub natural: bool,
    pub tension: bool,
}

/// What the processor holds and the audio thread reads: host parameter values (real units: choice index, integer,
/// 0/1, in [`host_params`] order), each musician's player seed, the feel switches, the song and the Library.
#[derive(Clone, Debug)]
pub struct BandSettings {
    pub host: [f32; HOST_PARAMS],
    pub seeds: [i32; NUM_MUSICIANS],
    pub feel: FeelFlags,
    pub song: Option<Arc<SongTimeline>>,
    pub library: Arc<LibraryData>,
    /// `chosenPlayer`, as a seed (0 = none): the player picked by name, which comes back when automation returns
    /// the instrument it plays (`handleAsyncUpdate`).
    chosen: [i32; NUM_MUSICIANS],
}

/// `buildTimeline (defaultSong())`, built once (the processor's constructor song; the runtime starts on it).
pub fn default_timeline() -> Arc<SongTimeline> {
    static T: OnceLock<Arc<SongTimeline>> = OnceLock::new();
    T.get_or_init(|| build_timeline(&LegacySong::default().to_song())).clone()
}

fn musician_param(k: usize, field: &str) -> Option<usize> {
    host_param_index(&format!("m{}_{field}", k + 1))
}

impl BandSettings {
    /// `MidiStudioProcessor (true)`'s state after its constructor: the host parameter defaults, `resetPlayers()`'s
    /// seeds, Natural and Tension on, and `buildTimeline (defaultSong())`.
    pub fn processor_default(lib: Arc<LibraryData>) -> Self {
        let mut host = [0.0f32; HOST_PARAMS];
        for (h, p) in host.iter_mut().zip(host_params()) {
            *h = p.def as f32;
        }
        let mut s = BandSettings {
            host,
            seeds: [0; NUM_MUSICIANS],
            feel: FeelFlags { natural: true, tension: true, ..FeelFlags::default() }, // a new session plays Natural ... and reacts to tension
            song: Some(default_timeline()),
            library: lib,
            chosen: [0; NUM_MUSICIANS],
        };
        s.reset_players();
        s
    }

    /// `firstPlayerSeed (inst)`: the instrument's first player (the Library's order).
    fn first_player_seed(&self, inst: i32) -> i32 {
        let Some(list) = usize::try_from(inst).ok().filter(|&i| i < K_INSTRUMENTS).and_then(|i| self.library.players.get(i)) else {
            return 0;
        };
        list.iter().copied().find(|&s| s > 0 && self.library.persona(s).is_some()).unwrap_or(0)
    }

    /// `resetPlayers()`: the factory band's players (and nothing missing).
    fn reset_players(&mut self) {
        let factory = crate::library::load::factory_player_names();
        for k in 0..NUM_MUSICIANS {
            let d = Band::factory_defaults().get(k).copied().unwrap_or_default();
            let mut seed = 0;
            for per in &self.library.personas {
                // the factory player in that slot, by name
                if factory.contains(&per.name) {
                    for &(inst, slot) in &per.slots {
                        if inst == d.instrument && slot == d.player {
                            seed = per.seed;
                        }
                    }
                }
            }
            if seed <= 0 && d.player >= 1 && d.player <= K_PLAYER_SLOTS as i32 {
                seed = usize::try_from(d.instrument)
                    .ok()
                    .and_then(|i| self.library.roster.get(i))
                    .and_then(|r| usize::try_from(d.player - 1).ok().and_then(|j| r.get(j)))
                    .copied()
                    .unwrap_or(0);
            }
            if seed <= 0 || self.library.persona(seed).is_none() {
                seed = self.first_player_seed(d.instrument); // every musician has a player
            }
            if let (Some(s), Some(c)) = (self.seeds.get_mut(k), self.chosen.get_mut(k)) {
                *s = seed;
                *c = seed;
            }
        }
    }

    fn instrument_of(&self, k: usize) -> i32 {
        musician_param(k, "instrument").and_then(|i| self.host.get(i)).map_or(0, |&v| round_to_int(v))
    }

    /// `setPlayerInternal`: "" or "Session" (or a name the Library lacks, or a player who doesn't play the
    /// musician's instrument) becomes the instrument's first player. False when the name was refused.
    fn set_player_internal(&mut self, k: usize, name: &str) -> bool {
        let name = name.trim();
        let inst = self.instrument_of(k);
        let first = self.first_player_seed(inst);
        let (ok, seed) = if name.is_empty() || name.eq_ignore_ascii_case("Session") {
            (true, first)
        } else {
            let seed = self.library.seed_for_name(name);
            // 0: not in the Library (missing); else it must play this instrument
            if seed == 0 || !self.library.plays(inst, seed) { (false, first) } else { (true, seed) }
        };
        if let Some(s) = self.seeds.get_mut(k) {
            *s = seed;
        }
        ok
    }

    /// MS `setPlayer (k, name)`: there is no Session player; "" / "Session" give the instrument's first player (MS
    /// `setPlayerInternal`), as does a name that isn't in the Library or doesn't play the slot's instrument (false).
    pub fn set_player(&mut self, k: usize, name: &str) -> bool {
        if k >= NUM_MUSICIANS {
            return false;
        }
        let ok = self.set_player_internal(k, name);
        let t = name.trim();
        let chosen = if t.is_empty() || t.eq_ignore_ascii_case("Session") { 0 } else { self.library.seed_for_name(t) };
        if let Some(c) = self.chosen.get_mut(k) {
            *c = chosen;
        }
        ok
    }

    /// MS `setTargetByName (k, name)`: the target by name (false if the Library has none), then its Keyswitch
    /// Octave default (`applyTargetKsDefault`).
    pub fn set_target(&mut self, k: usize, name: &str) -> bool {
        if k >= NUM_MUSICIANS {
            return false;
        }
        let slot = self.library.target_slot_for_name(name);
        if slot < 0 {
            return false;
        }
        self.set_param(&format!("m{}_target", k + 1), slot as f32);
        let Some(t) = musician_param(k, "target").and_then(|i| self.host.get(i)).map(|&v| round_to_int(v)) else { return true };
        if let Some(target) = self.library.target(t) {
            let ks = target.ks_octave_default as f32;
            self.set_param(&format!("m{}_ksoctave", k + 1), ks);
        }
        true
    }

    /// `setParamValue (id, v)`: the value as the host parameter stores it (clamped to its range; Int and Choice
    /// rounded as JUCE rounds, Bool snapped to 0 / 1). An instrument change re-picks players (`handleAsyncUpdate`).
    /// False for an unknown id.
    pub fn set_param(&mut self, id: &str, v: f32) -> bool {
        let Some(i) = host_param_index(id) else { return false };
        let Some(p) = host_params().get(i) else { return false };
        let (lo, hi) = (p.min as f32, p.max as f32);
        let v = if v.is_nan() { p.def as f32 } else { v.clamp(lo, hi) };
        let stored = match p.kind {
            HostParamKind::Int | HostParamKind::Choice => round_to_int(v) as f32,
            HostParamKind::Bool => {
                if v >= 0.5 {
                    1.0
                } else {
                    0.0
                }
            } // AudioParameterBool snaps
        };
        if let Some(h) = self.host.get_mut(i) {
            *h = stored;
        }
        if id.len() > 3 && id.ends_with("_instrument") {
            self.instrument_changed();
        }
        true
    }

    /// `handleAsyncUpdate`'s player check: each player still plays its musician's instrument.
    fn instrument_changed(&mut self) {
        for k in 0..NUM_MUSICIANS {
            let seed = self.seeds.get(k).copied().unwrap_or(0);
            let inst = self.instrument_of(k);
            let ws = self.chosen.get(k).copied().unwrap_or(0);
            if ws > 0 && self.library.plays(inst, ws) {
                if seed != ws {
                    // the instrument came back: the chosen player
                    if let Some(s) = self.seeds.get_mut(k) {
                        *s = ws;
                    }
                }
            } else if seed <= 0 || !self.library.plays(inst, seed) {
                let first = self.first_player_seed(inst); // the new instrument's first player
                if let Some(s) = self.seeds.get_mut(k) {
                    *s = first;
                }
            }
        }
    }

    /// A host parameter's current value (real units), by id.
    pub fn param(&self, id: &str) -> Option<f32> {
        self.host.get(host_param_index(id)?).copied()
    }
}

/// Old `Arc`s the audio thread hands back to be freed elsewhere (`retiredSong` / `retiredLibrary`).
#[derive(Debug, Default)]
pub struct Retired {
    pub songs: [Option<Arc<SongTimeline>>; 4],
    pub libs: [Option<Arc<LibraryData>>; 2],
}

impl Retired {
    pub fn is_empty(&self) -> bool {
        self.songs.iter().all(Option::is_none) && self.libs.iter().all(Option::is_none)
    }
    fn put<T>(slots: &mut [Option<T>], v: T) -> Result<(), T> {
        match slots.iter_mut().find(|s| s.is_none()) {
            Some(s) => {
                *s = Some(v);
                Ok(())
            }
            None => Err(v),
        }
    }
}

/// The runtime's own holding area for retired `Arc`s until the next [`BandRuntime::apply`] hands them out.
#[derive(Debug, Default)]
struct Held {
    songs: [Option<Arc<SongTimeline>>; 8],
    libs: [Option<Arc<LibraryData>>; 4],
    /// `Arc`s leaked because even this was full (never expected; read by tests)
    leaked: usize,
}

impl Held {
    fn is_empty(&self) -> bool {
        self.songs.iter().all(Option::is_none) && self.libs.iter().all(Option::is_none)
    }
    /// Never frees: when even this is full (a caller that never empties its `Retired`), the `Arc` is leaked.
    fn song(&mut self, a: Arc<SongTimeline>) {
        if let Err(a) = Retired::put(&mut self.songs, a) {
            std::mem::forget(a);
            self.leaked += 1;
        }
    }
    fn lib(&mut self, a: Arc<LibraryData>) {
        if let Err(a) = Retired::put(&mut self.libs, a) {
            std::mem::forget(a);
            self.leaked += 1;
        }
    }
    fn drain_into(&mut self, r: &mut Retired) {
        for s in &mut self.songs {
            if let Some(a) = s.take()
                && let Err(a) = Retired::put(&mut r.songs, a)
            {
                *s = Some(a);
            }
        }
        for s in &mut self.libs {
            if let Some(a) = s.take()
                && let Err(a) = Retired::put(&mut r.libs, a)
            {
                *s = Some(a);
            }
        }
    }
}

fn same_song(a: &Option<Arc<SongTimeline>>, b: &Option<Arc<SongTimeline>>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => Arc::ptr_eq(x, y),
        (None, None) => true,
        _ => false,
    }
}

/// `BufferSink`: engine events -> the host. Rendered (export) events never reach the host; offsets are clamped to
/// the block.
struct BlockSink<'a> {
    out: &'a mut dyn MidiSink,
    frames: usize,
}

impl MidiSink for BlockSink<'_> {
    fn event(&mut self, e: &MidiEvent) {
        if e.rendered {
            return;
        }
        let hi = i32::try_from(self.frames.saturating_sub(1)).unwrap_or(i32::MAX);
        let c = MidiEvent { offset: e.offset.clamp(0, hi), ..*e };
        self.out.event(&c);
    }
}

pub struct BandRuntime {
    band: Band,
    /// `preparedRate`
    sr: f64,
    /// the host parameter values the audio thread reads (MS `raw`)
    host: [f32; HOST_PARAMS],
    last_raw: [f32; HOST_PARAMS],
    force_push: bool,
    packed_now: [f32; PACKED],
    packed_last: [f32; PACKED],
    /// packed endpoint -> Band route
    packed_route: [usize; PACKED],
    have_packed: bool,
    has_played: bool,
    seeds: [i32; NUM_MUSICIANS],
    seeds_changed: bool,
    feel: FeelFlags,
    /// `pendingSong`: Some(new) waits for step 2a (the inner None = no song)
    pending_song: Option<Option<Arc<SongTimeline>>>,
    pending_lib: Option<Arc<LibraryData>>,
    /// settings that arrived while the caller's `Retired` was full: applied at the next block
    deferred: Option<BandSettings>,
    held: Held,
    /// note-offs owed from a re-prepare
    carry: VecSink,
    performer: Performer,
}

impl BandRuntime {
    /// The processor (constructor, then `prepareToPlay (sr, …)`) with `s` applied as the processor's setters would
    /// leave it: the band starts on the default song, and `s`'s song, parameters and players go in on the first block.
    pub fn new(sr: f64, s: BandSettings) -> BandRuntime {
        let mut band = Band::new(s.library.clone());
        let _ = band.set_song(Some(default_timeline())); // before any audio: then only through pendingSong
        let mut packed_route = [0usize; PACKED];
        for (r, e) in packed_route.iter_mut().zip(packed_endpoints()) {
            *r = Band::endpoint_index(&e.name).unwrap_or(usize::MAX);
        }
        let mut rt = BandRuntime {
            band,
            sr: 48000.0,
            host: [0.0; HOST_PARAMS],
            last_raw: [0.0; HOST_PARAMS],
            force_push: true,
            packed_now: [0.0; PACKED],
            packed_last: [0.0; PACKED],
            packed_route,
            have_packed: false,
            has_played: false,
            seeds: [0; NUM_MUSICIANS],
            seeds_changed: true,
            feel: FeelFlags::default(),
            pending_song: None,
            pending_lib: None,
            deferred: None,
            held: Held::default(),
            carry: VecSink { events: Vec::with_capacity(512), markers: Vec::with_capacity(16) },
            performer: Performer::default(),
        };
        rt.apply_now(s);
        rt.seeds_changed = true;
        rt.prepare(sr);
        rt
    }

    /// `prepareToPlay`: the band back to the graph's initial state (what it was sounding is closed into `carry`, sent
    /// at the start of the next block), and every parameter goes again on the first block.
    pub fn prepare(&mut self, sr: f64) {
        if self.has_played {
            self.carry.events.clear();
            self.band.flush(&mut self.carry);
        }
        self.sr = sr;
        self.band.prepare(sr);
        self.force_push = true;
        self.have_packed = false;
    }

    /// `processBlockBypassed`'s first block (or a hard stop): close everything the band is sounding into `out`.
    pub fn flush(&mut self, out: &mut dyn MidiSink) {
        self.band.flush(out);
    }

    /// New settings from off the audio thread. No allocation; the `Arc`s it displaces (and the ones the last blocks
    /// swapped out) go into `retired`. When `retired` has no room, the newest settings wait whole and apply at the
    /// next block.
    pub fn apply(&mut self, s: BandSettings, retired: &mut Retired) {
        self.held.drain_into(retired);
        if !self.held.is_empty() {
            // The caller's Retired is full: keep the newest settings pending, applied at the next block.
            if let Some(old) = self.deferred.replace(s) {
                self.retire_settings(old);
            }
            return;
        }
        if let Some(old) = self.deferred.take() {
            self.retire_settings(old);
        }
        self.apply_now(s);
        self.held.drain_into(retired);
    }

    /// The song the band will have after the pending swap.
    fn song_target(&self) -> &Option<Arc<SongTimeline>> {
        self.pending_song.as_ref().unwrap_or_else(|| self.band.song_option())
    }
    /// The Library the band will have after the pending swap.
    fn lib_target(&self) -> &Arc<LibraryData> {
        self.pending_lib.as_ref().unwrap_or_else(|| self.band.library())
    }

    /// An `Arc` leaving the runtime. One that is the same allocation as the pending or playing song (Library) is
    /// simply dropped: the runtime still holds that allocation, so the drop only decrements a count. Any other waits
    /// in `held` for the next `apply` to hand it out through `Retired`.
    fn park_song(&mut self, a: Arc<SongTimeline>) {
        let live = |o: &Option<Arc<SongTimeline>>| o.as_ref().is_some_and(|x| Arc::ptr_eq(x, &a));
        if self.pending_song.as_ref().is_some_and(live) || live(self.band.song_option()) {
            return;
        }
        self.held.song(a);
    }
    fn park_lib(&mut self, a: Arc<LibraryData>) {
        if self.pending_lib.as_ref().is_some_and(|x| Arc::ptr_eq(x, &a)) || Arc::ptr_eq(self.band.library(), &a) {
            return;
        }
        self.held.lib(a);
    }

    /// Test support: how many `Arc`s were leaked because the holding area was full (0 unless a caller never empties
    /// its `Retired`).
    #[doc(hidden)]
    pub fn leaked_arcs(&self) -> usize {
        self.held.leaked
    }

    /// Settings that never reached the band: their `Arc`s (those not shared with what is pending or playing) wait
    /// in `held`.
    fn retire_settings(&mut self, s: BandSettings) {
        let BandSettings { song, library, .. } = s;
        if let Some(a) = song {
            self.park_song(a);
        }
        self.park_lib(library);
    }

    fn apply_now(&mut self, s: BandSettings) {
        let BandSettings { host, seeds, feel, song, library, .. } = s;
        self.host = host;
        if seeds != self.seeds {
            self.seeds = seeds;
            self.seeds_changed = true;
        }
        self.feel = feel;
        // The same song / Library as the one pending or playing: dropping this handle only decrements a count that
        // stays above zero (the band or the pending slot holds the same allocation), so nothing is freed here.
        if !same_song(&song, self.song_target())
            && let Some(Some(old)) = self.pending_song.replace(song)
        {
            self.park_song(old);
        }
        if !Arc::ptr_eq(&library, self.lib_target())
            && let Some(old) = self.pending_lib.replace(library)
        {
            self.park_lib(old);
        }
    }

    /// `processBlock` steps 0, 1, 2, 2a, 2b, 3, 4 (Live off) and 5 for one block of `frames` samples.
    pub fn process_block(&mut self, t: &HostTransport, frames: usize, out: &mut dyn MidiSink) {
        if let Some(s) = self.deferred.take() {
            self.apply_now(s);
        }
        let n = frames;
        let mut sink = BlockSink { out, frames: n };
        self.has_played = true;

        // 0. Note-offs owed from a re-prepare.
        for e in &self.carry.events {
            sink.event(&MidiEvent { offset: 0, ..*e });
        }
        self.carry.events.clear();

        // 1. A reloaded Library: every musician closes its notes, then the swap. (MS: under applyLock / libraryLock,
        //    with `auditionK = -1`; the audition is not ported.)
        if let Some(fresh) = self.pending_lib.take() {
            let old = self.band.set_library(fresh, &mut sink); // the old one is freed off the audio thread, not here
            self.park_lib(old);
        }

        // 2. Parameters: fold to engine endpoints only when something moved, then send what changed.
        let mut changed = std::mem::replace(&mut self.force_push, false);
        for (last, &v) in self.last_raw.iter_mut().zip(&self.host) {
            if v != *last {
                *last = v;
                changed = true;
            }
        }
        if changed {
            let all = !self.have_packed;
            pack_into(&self.last_raw, &mut self.packed_now); // allocation-free (params.rs)
            for i in 0..PACKED {
                if all || self.packed_last[i] != self.packed_now[i] {
                    self.band.set_param_at(self.packed_route[i], self.packed_now[i]);
                    self.packed_last[i] = self.packed_now[i];
                }
            }
            self.have_packed = true;
        }

        // 2a. An edited song: swapped in here (the old timeline is freed off the audio thread).
        if let Some(song) = self.pending_song.take()
            && let Some(old) = self.band.set_song(song)
        {
            self.park_song(old);
        }

        // 2b. Players (by seed): on change, and again after a re-prepare rebuilt the musicians.
        if std::mem::replace(&mut self.seeds_changed, false) || changed {
            for (k, &seed) in self.seeds.iter().enumerate() {
                self.band.set_persona_seed(k, seed);
            }
        }

        // 3. Transport (read first: incoming notes are placed on it).
        let (playing, bpm, ppq, num, den) = (t.playing, t.bpm, t.ppq, t.num, t.den);
        let sr = 1.0f64.max(self.sr);
        let ppq_per_sample = bpm / 60.0 / sr; // the rate the engine was prepared with
        // The bar and beat the engine plays (the song's meter, else the host's; unsupported meters play as 4/4).
        let (host_meter, _unsupported) = meter_from_host(num, den);
        let play_meter = match self.band.song_now() {
            Some(s) if s.meter_override >= 0 => Meter::from_index(s.meter_override),
            _ => host_meter,
        };
        let bar_len = f64::from(info(play_meter).bar_ppq);
        let beat_len = if play_meter == Meter::M68 { 1.5 } else { bar_len / f64::from(info(play_meter).beats_per_bar) };
        // (hostBarLen / hostNum / hostDen: the UI's, not ported.)

        // 3a. Groove audition (PluginProcessor.cpp 234-269): not ported (no audition in the DAW).

        // 4. Incoming MIDI and the Performer, with Live off (round 1): no incoming MIDI (278-288), no jump requests
        //    (289), A/B off (290-292: stopAB never runs).
        self.performer.set_settings(false, false, 4, 24);
        self.performer.set_mod_wheel(self.feel.mod_wheel);
        self.performer.set_interplay(self.feel.bass_lock, self.feel.fills_yield, self.feel.leave_space, self.feel.dynamics);
        self.performer.set_natural(self.feel.natural);
        self.performer.set_tension(self.feel.tension);
        self.performer.set_key(round_to_int(self.last_raw[0]), round_to_int(self.last_raw[1])); // key, scale
        self.performer.set_ab(false, '\0', 1, 1);
        self.performer.set_loop('\0');
        if let Some(song) = self.band.song_now() {
            let end = ppq + n as f64 * ppq_per_sample;
            self.performer.advance(playing, ppq, end, bar_len, beat_len, song);
        }
        let ps = *self.performer.state();
        self.band.set_performance(&ps);
        // The cue / Live chord / recording / UI feed (297-340): the UI's, not ported.
        self.band.set_host_transport(playing, bpm, ppq, num, den);

        // 5. Play. (The standalone's hosted instruments and the UI feed, 345-356: not ported.)
        self.band.process(n, &mut sink);
    }

    pub fn band(&self) -> &Band {
        &self.band
    }

    /// Test support: (address, capacity) of every buffer the runtime and its band own.
    #[doc(hidden)]
    pub fn buffer_addresses(&self) -> Vec<(usize, usize)> {
        let mut v = self.band.buffer_addresses();
        v.push((self.carry.events.as_ptr() as usize, self.carry.events.capacity()));
        v.push((self.carry.markers.as_ptr() as usize, self.carry.markers.capacity()));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::load::embedded_library;

    #[test]
    fn processor_default_players_are_the_factory_band() {
        let s = BandSettings::processor_default(embedded_library());
        let name = |seed: i32| s.library.persona(seed).map(|p| p.name.clone()).unwrap_or_default();
        // Band.h factoryDefaults: Piano slot 1, Drums 1, Bass 7, Strings 1, Brass 1; m6 Piano slot 0 -> first player.
        assert_eq!(name(s.seeds[0]), "Clay");
        assert_eq!(name(s.seeds[1]), "Benny");
        assert_eq!(name(s.seeds[2]), "Cole");
        assert_eq!(name(s.seeds[3]), "Aria");
        assert_eq!(name(s.seeds[4]), "Duke");
        assert!(s.seeds[5] > 0);
        assert!(s.feel.natural && s.feel.tension && !s.feel.bass_lock);
        assert_eq!(s.param("m1_target"), Some(4.0));
    }

    #[test]
    fn setters_follow_ms() {
        let mut s = BandSettings::processor_default(embedded_library());
        assert!(s.set_player(2, "Otis"));
        assert!(!s.set_player(2, "Clay")); // Clay doesn't play bass: the bass's first player
        assert!(!s.set_player(2, "Nobody"));
        assert!(s.set_player(2, "Session"));
        assert!(s.set_target(0, "Pianoteq"));
        assert_eq!(s.param("m1_target"), Some(3.0));
        assert!(!s.set_target(0, "Nope"));
        assert!(s.set_param("m1_energy", 9.0));
        assert_eq!(s.param("m1_energy"), Some(4.0)); // clamped to the range
        assert!(!s.set_param("m9_energy", 1.0));
        // An instrument change re-picks the player; the chosen player comes back with the instrument.
        assert!(s.set_player(0, "Selah"));
        let selah = s.seeds[0];
        assert!(s.set_param("m1_instrument", 6.0));
        assert_ne!(s.seeds[0], selah);
        assert!(s.set_param("m1_instrument", 0.0));
        assert_eq!(s.seeds[0], selah);
    }
}
