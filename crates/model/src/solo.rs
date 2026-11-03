//! Solo follows the signal path.
//!
//! With any active track soloed, a track that is neither soloed nor solo safe stays audible when
//! it is on a soloed track's signal path:
//! - **upstream**: it feeds a soloed track through main outputs, transitively (Kick In → Kick
//!   Bus when the Kick Bus is soloed). Sends don't count here: soloing a reverb return must not
//!   unmute every track that sends to it;
//! - **downstream**: a soloed track reaches it through outputs *or* sends, transitively (the
//!   busses a soloed track plays through, and its reverb return).
//!
//! A track "feeds" another when its output (or send) targets the bus that is the other's input.
//! Routing folders are auxes on their own bus, so folder members follow the same rule. Inactive
//! tracks neither solo nor carry a path. Routing cycles are safe: every pass marks at least one
//! more track or stops, so there are at most as many passes as tracks.
//!
//! The mix engine and the UI share [`SoloScope`] so a strip's implied mute is exactly what the
//! engine plays.

use std::collections::HashMap;

use crate::{BusId, Route, Session, Track};

/// Which tracks a solo leaves audible. Make it with [`SoloScope::reserve`] (allocates, off the
/// audio thread); [`SoloScope::fill`] then doesn't allocate for the same session shape.
#[derive(Debug, Default, Clone)]
pub struct SoloScope {
    /// Any active track is soloed.
    any: bool,
    /// Indexed like `s.tracks`: soloed, solo safe, or on a soloed track's path.
    audible: Vec<bool>,
    /// Indexed like `s.tracks`: reached downstream (while filling).
    down: Vec<bool>,
    /// Indexed like `s.tracks`: reached upstream (while filling).
    up: Vec<bool>,
    /// Position of each bus in `s.busses`.
    bus_at: HashMap<BusId, usize>,
    /// Indexed like `s.busses` (while filling).
    flags: Vec<bool>,
    /// Indexed like `s.tracks`: the soloed track whose path reaches it ([`NONE`] when none; a
    /// soloed track is its own cause).
    cause: Vec<u32>,
    /// Indexed like `s.busses` (while filling): the cause of the first track that marked it.
    bus_cause: Vec<u32>,
}

/// No cause.
const NONE: u32 = u32::MAX;

impl SoloScope {
    /// A filled scope for `s` (allocates: for the UI and tests).
    pub fn of(s: &Session) -> Self {
        let mut scope = SoloScope::default();
        scope.reserve(s);
        scope.fill(s);
        scope
    }

    /// Room for the tracks and busses of `s`. Allocates (only what grew or changed).
    pub fn reserve(&mut self, s: &Session) {
        let n = s.tracks.len();
        for v in [&mut self.audible, &mut self.down, &mut self.up] {
            v.clear();
            v.reserve(n);
        }
        self.cause.clear();
        self.cause.reserve(n);
        self.bus_at.clear();
        self.bus_at.extend(s.busses.iter().enumerate().map(|(i, b)| (b.id, i)));
        if self.flags.len() != s.busses.len() {
            self.flags = vec![false; s.busses.len()];
            self.bus_cause = vec![NONE; s.busses.len()];
        }
    }

    /// Recomputes the scope for `s`. Linear in the tracks per pass; doesn't allocate once
    /// reserved for `s` (a session that grew since only costs the growth).
    pub fn fill(&mut self, s: &Session) {
        let SoloScope { any, audible, down, up, bus_at, flags, cause, bus_cause } = self;
        let soloed = |t: &Track| t.mixer.solo && !t.inactive;
        *any = s.tracks.iter().any(soloed);
        audible.clear();
        down.clear();
        up.clear();
        cause.clear();
        if !*any {
            return;
        }
        down.extend(s.tracks.iter().map(soloed));
        up.extend(down.iter().copied());
        cause.extend(down.iter().enumerate().map(|(i, on)| if *on { u32::try_from(i).unwrap_or(NONE) } else { NONE }));
        let bus = |r: &Route| match r {
            Route::Bus(b) => bus_at.get(b).copied(),
            _ => None,
        };
        // The cause of the track that first marked the bus `r` targets, when one did.
        let marked = |flags: &[bool], bus_cause: &[u32], r: &Route| {
            bus(r).filter(|i| flags.get(*i).copied().unwrap_or(false)).map(|i| bus_cause.get(i).copied().unwrap_or(NONE))
        };
        // Marks the bus `r` targets with `why`, unless already marked this pass.
        let mark = |flags: &mut [bool], bus_cause: &mut [u32], r: &Route, why: u32| {
            if let Some(i) = bus(r)
                && let (Some(x), Some(c)) = (flags.get_mut(i), bus_cause.get_mut(i))
                && !*x
            {
                *x = true;
                *c = why;
            }
        };
        // Downstream: the busses the reached tracks play into (outputs and sends) reach the
        // tracks they are the input of.
        for _ in 0..s.tracks.len() {
            flags.iter_mut().for_each(|x| *x = false);
            for (i, t) in s.tracks.iter().enumerate().filter(|(i, t)| down.get(*i).copied().unwrap_or(false) && !t.inactive) {
                let why = cause.get(i).copied().unwrap_or(NONE);
                for r in std::iter::once(&t.mixer.output).chain(t.mixer.sends.iter().flatten().map(|sn| &sn.target)) {
                    mark(flags, bus_cause, r, why);
                }
            }
            let mut grew = false;
            for ((t, on), c) in s.tracks.iter().zip(down.iter_mut()).zip(cause.iter_mut()) {
                if !*on
                    && !t.inactive
                    && let Some(why) = marked(flags, bus_cause, &t.mixer.input)
                {
                    *on = true;
                    if *c == NONE {
                        *c = why;
                    }
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        // Upstream: the input busses of the reached tracks reach the tracks whose main output
        // plays into them.
        for _ in 0..s.tracks.len() {
            flags.iter_mut().for_each(|x| *x = false);
            for (i, t) in s.tracks.iter().enumerate().filter(|(i, t)| up.get(*i).copied().unwrap_or(false) && !t.inactive) {
                mark(flags, bus_cause, &t.mixer.input, cause.get(i).copied().unwrap_or(NONE));
            }
            let mut grew = false;
            for ((t, on), c) in s.tracks.iter().zip(up.iter_mut()).zip(cause.iter_mut()) {
                if !*on
                    && !t.inactive
                    && let Some(why) = marked(flags, bus_cause, &t.mixer.output)
                {
                    *on = true;
                    if *c == NONE {
                        *c = why;
                    }
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        audible.extend(s.tracks.iter().zip(down.iter().zip(up.iter())).map(|(t, (d, u))| *d || *u || t.mixer.solo_safe));
    }

    /// Any active track is soloed.
    pub fn any_solo(&self) -> bool {
        self.any
    }

    /// Whether the solo mutes track `i` (an index into `s.tracks`): a track is soloed and `i` is
    /// neither soloed, solo safe nor on a soloed track's signal path. An index the scope doesn't
    /// know is muted while any track is soloed.
    pub fn muted(&self, i: usize) -> bool {
        self.any && !self.audible.get(i).copied().unwrap_or(false)
    }

    /// Track `i` is soloed itself (and active).
    pub fn soloed(&self, i: usize) -> bool {
        self.any && self.cause.get(i).is_some_and(|c| usize::try_from(*c).ok() == Some(i))
    }

    /// Track `i` is not soloed but plays because it is on a soloed track's signal path (an
    /// implied solo). A solo safe track off every path is not implied.
    pub fn implied(&self, i: usize) -> bool {
        self.cause(i).is_some_and(|c| c != i)
    }

    /// The soloed track (an index into `s.tracks`) whose path keeps track `i` playing: `i`
    /// itself when soloed, `None` when no solo reaches it.
    pub fn cause(&self, i: usize) -> Option<usize> {
        if !self.any {
            return None;
        }
        self.cause.get(i).filter(|c| **c != NONE).and_then(|c| usize::try_from(*c).ok())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::{ChannelFormat, SendSlot, TrackId, TrackKind};

    struct Kit {
        s: Session,
        kick_in: TrackId,
        kick_out: TrackId,
        snare: TrackId,
        kick_bus: TrackId,
        drum_bus: TrackId,
        verb: TrackId,
    }

    /// Kick In, Kick Out → Kick Bus → Drum Bus → Main; Snare → Drum Bus and sends to Verb.
    fn kit() -> Kit {
        let mut s = Session::default();
        let audio = |s: &mut Session, name: &str| s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(name));
        let kick_in = audio(&mut s, "Kick In");
        let kick_out = audio(&mut s, "Kick Out");
        let snare = audio(&mut s, "Snare");
        let (kb, db, vb) =
            (s.add_bus("Kick Bus", ChannelFormat::Stereo), s.add_bus("Drum Bus", ChannelFormat::Stereo), s.add_bus("Verb", ChannelFormat::Stereo));
        let aux = |s: &mut Session, name: &str, input: BusId, output: Route| {
            let t = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some(name));
            let m = &mut s.track_mut(t).unwrap().mixer;
            m.input = Route::Bus(input);
            m.output = output;
            t
        };
        let kick_bus = aux(&mut s, "Kick Bus", kb, Route::Bus(db));
        let drum_bus = aux(&mut s, "Drum Bus", db, Route::Main);
        let verb = aux(&mut s, "Verb", vb, Route::Main);
        s.track_mut(kick_in).unwrap().mixer.output = Route::Bus(kb);
        s.track_mut(kick_out).unwrap().mixer.output = Route::Bus(kb);
        let sn = &mut s.track_mut(snare).unwrap().mixer;
        sn.output = Route::Bus(db);
        sn.sends[0] = Some(SendSlot::new(Route::Bus(vb)));
        Kit { s, kick_in, kick_out, snare, kick_bus, drum_bus, verb }
    }

    fn muted(s: &Session, ids: &[TrackId]) -> Vec<bool> {
        let scope = SoloScope::of(s);
        ids.iter().map(|id| scope.muted(s.track_index(*id).unwrap())).collect()
    }

    fn solo(s: &mut Session, t: TrackId) {
        s.track_mut(t).unwrap().mixer.solo = true;
    }

    #[test]
    fn nothing_soloed_mutes_nothing() {
        let k = kit();
        let all = [k.kick_in, k.kick_out, k.snare, k.kick_bus, k.drum_bus, k.verb];
        assert!(!SoloScope::of(&k.s).any_solo());
        assert_eq!(muted(&k.s, &all), vec![false; 6]);
    }

    #[test]
    fn soloing_a_bus_keeps_its_sources_and_destinations() {
        let mut k = kit();
        solo(&mut k.s, k.kick_bus);
        let ids = [k.kick_in, k.kick_out, k.kick_bus, k.drum_bus, k.snare, k.verb];
        assert_eq!(muted(&k.s, &ids), vec![false, false, false, false, true, true]);
    }

    #[test]
    fn soloing_a_source_keeps_its_busses_only() {
        let mut k = kit();
        solo(&mut k.s, k.kick_in);
        let ids = [k.kick_in, k.kick_bus, k.drum_bus, k.kick_out, k.snare, k.verb];
        assert_eq!(muted(&k.s, &ids), vec![false, false, false, true, true, true]);
    }

    #[test]
    fn sends_reach_downstream_but_never_upstream() {
        let mut k = kit();
        solo(&mut k.s, k.verb);
        assert_eq!(muted(&k.s, &[k.verb, k.snare, k.drum_bus]), vec![false, true, true]);
        k.s.track_mut(k.verb).unwrap().mixer.solo = false;
        solo(&mut k.s, k.snare);
        assert_eq!(muted(&k.s, &[k.snare, k.drum_bus, k.verb, k.kick_bus, k.kick_in]), vec![false, false, false, true, true]);
    }

    #[test]
    fn solo_safe_and_inactive() {
        let mut k = kit();
        solo(&mut k.s, k.kick_in);
        k.s.track_mut(k.snare).unwrap().mixer.solo_safe = true;
        assert_eq!(muted(&k.s, &[k.snare]), vec![false]);
        // An inactive soloed track solos nothing.
        k.s.track_mut(k.kick_in).unwrap().inactive = true;
        assert!(!SoloScope::of(&k.s).any_solo());
    }

    #[test]
    fn routing_cycles_terminate() {
        let mut k = kit();
        // Drum Bus back into the Kick Bus, and the Verb into itself.
        let kb = k.s.track(k.kick_bus).unwrap().mixer.input.clone();
        k.s.track_mut(k.drum_bus).unwrap().mixer.output = kb;
        let vb = k.s.track(k.verb).unwrap().mixer.input.clone();
        k.s.track_mut(k.verb).unwrap().mixer.output = vb;
        solo(&mut k.s, k.kick_bus);
        assert_eq!(muted(&k.s, &[k.kick_in, k.drum_bus, k.snare, k.verb]), vec![false, false, false, true]);
        k.s.track_mut(k.kick_bus).unwrap().mixer.solo = false;
        solo(&mut k.s, k.verb);
        assert_eq!(muted(&k.s, &[k.verb, k.snare]), vec![false, true]);
    }

    #[test]
    fn implied_solos_name_the_soloed_track() {
        let mut k = kit();
        solo(&mut k.s, k.kick_bus);
        let scope = SoloScope::of(&k.s);
        let ids = k.s.clone();
        let at = |id: TrackId| ids.track_index(id).unwrap();
        // Kick In/Out (upstream) and the Drum Bus (downstream) play because of the Kick Bus.
        for id in [k.kick_in, k.kick_out, k.drum_bus] {
            assert!(scope.implied(at(id)), "{id:?}");
            assert_eq!(scope.cause(at(id)), Some(at(k.kick_bus)));
        }
        // The soloed track itself is soloed, not implied; muted tracks are neither.
        assert!(!scope.implied(at(k.kick_bus)) && scope.soloed(at(k.kick_bus)));
        assert!(!scope.implied(at(k.snare)) && !scope.soloed(at(k.snare)) && scope.muted(at(k.snare)));
        assert_eq!(scope.cause(at(k.snare)), None);
        // Solo safe is not implied: it plays because it is safe, not because of the path.
        k.s.track_mut(k.verb).unwrap().mixer.solo_safe = true;
        let scope = SoloScope::of(&k.s);
        assert!(!scope.implied(at(k.verb)) && !scope.muted(at(k.verb)));
        // Nothing soloed: nothing implied.
        let none = SoloScope::of(&kit().s);
        assert!((0..6).all(|i| !none.implied(i) && !none.soloed(i)));
    }

    #[test]
    fn filling_again_does_not_allocate() {
        let mut k = kit();
        solo(&mut k.s, k.kick_bus);
        let mut scope = SoloScope::default();
        scope.reserve(&k.s);
        scope.fill(&k.s);
        let caps = (scope.audible.capacity(), scope.down.capacity(), scope.up.capacity(), scope.flags.capacity(), scope.cause.capacity());
        k.s.track_mut(k.kick_bus).unwrap().mixer.solo = false;
        solo(&mut k.s, k.snare);
        scope.fill(&k.s);
        assert_eq!(caps, (scope.audible.capacity(), scope.down.capacity(), scope.up.capacity(), scope.flags.capacity(), scope.cause.capacity()));
    }
}
