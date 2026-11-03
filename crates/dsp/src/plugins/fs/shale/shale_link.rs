//! Kit Link from `Shale/Source/ShaleLink.{h,cpp}` and its shared audio, `ShaleAudioShare.{h,cpp}`,
//! on the host's link group slab ([`LinkSlab`], plan B4) instead of the C++'s memory-mapped files.
//!
//! What decides the sound, translated as the C++:
//! - [`LinkEvent`], the event as the C++ stores it (`Event`);
//! - [`same_drum`] and [`is_partner`], which decide whose TRIGGERS open this gate (partners) and
//!   whose BEACONS argue against it (neighbours), exactly as `ShaleLink::collect`;
//! - [`collect_from`], `collect`'s per-slot filter (same clock, inside the window, partners give
//!   triggers only, neighbours give beacons only).
//!
//! The wrapper side (block-relative offsets, the 12 ms reach back, own triggers only, Follow never
//! publishing) lives in `fs_shale`'s `link_in` / `link_out`.
//!
//! The transport, [`KitLink`]: the C++ design kept, one slot per instance (at most
//! [`MAX_INSTANCES`] = 16), each instance writing only its own slot (presence, a seqlocked ring of
//! its last 64 events, and in a second region the last 64k samples of its mono input, stamped with
//! the timeline positions of its unbroken run). What maps where:
//! - **The files** (`link.dat`, `audio.dat`, one each per machine) are the session's link group
//!   (`Insert::link_group`): the modules the user puts in one group share a slab, on any track. The
//!   slab's [`LinkSlab::layout`] stands for the files' magic and version: a group that mixes Shale
//!   with another plugin is "from another version", and Kit Link is unavailable, as in the C++.
//! - **A slot** is the region of this member's index (stable while the module stays in the group);
//!   an index past 15 has no slot ("all 16 link slots are in use").
//! - **Claiming and releasing** (`claimSlot`, the destructor) happen when the host hands the plugin
//!   a slab or takes it away ([`crate::Plugin::set_link`]) and when the plugin is dropped. A release
//!   clears the slot only while this instance still owns it (a token in the slot), so an instance
//!   retired after a newcomer took its index never clears the newcomer's slot.
//! - **The heartbeat** (`keepAlive`, 4 s staleness): the host knows who is in the group, so a slot
//!   is live while its instance has Kit Link on (published every block, and withdrawn the block it
//!   is switched off rather than four seconds later). An instance the host does not call (a module
//!   bypassed in the strip) keeps its last presence, as the C++ timer keeps a silent one alive.
//! - **The clock** is always the host's sample timeline (`Transport.samples`): the DAW always gives
//!   positions, so `clkWall` never occurs (the constant is kept for the event format).
//!
//! The slab is sized once, when the group is made: [`WORDS`] for every Shale (all 16 slots of both
//! regions, about 4.2 MB), so a group of Shales never has to grow it.

use super::shale_dsp::{I_KICK_IN, I_SNARE_BTM, I_SNARE_TOP, I_SUB_KICK};
use crate::link::LinkSlab;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// `ClockKind`: which timeline an event's position is on.
pub const CLK_TRANSPORT: i32 = 0;
pub const CLK_WALL: i32 = 1;

/// `Role` (matches `ShaleDSP::LinkRole`).
pub const ROLE_LEAD: i32 = 0;
pub const ROLE_FOLLOW: i32 = 1;
pub const ROLE_PAIR: i32 = 2;

/// `EventKind`.
pub const EV_BEACON: i32 = 0;
pub const EV_TRIGGER: i32 = 1;

/// `Event`: one beacon or trigger on the shared timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinkEvent {
    /// Sample position on the clock named by `clock`.
    pub pos: i64,
    /// Beacons only: peak of the transient, 60..1200 Hz.
    pub level_db: f32,
    /// Triggers only.
    pub velocity: f32,
    pub kind: i32,
    pub clock: i32,
}

impl Default for LinkEvent {
    fn default() -> Self {
        LinkEvent { pos: 0, level_db: -120.0, velocity: 0.0, kind: EV_BEACON, clock: CLK_WALL }
    }
}

/// `ShaleLink::sameDrum`: Kick In / Out / Sub, Snare Top / Btm, otherwise the same mic.
pub fn same_drum(a: i32, b: i32) -> bool {
    let drum = |i: i32| {
        if (I_KICK_IN..=I_SUB_KICK).contains(&i) {
            0
        } else if i == I_SNARE_TOP || i == I_SNARE_BTM {
            1
        } else {
            2 + i
        }
    };
    drum(a) == drum(b)
}

/// Whether another instance (role `their_role`, mic `their_instrument`) is this one's PARTNER
/// (`collect`): a Pair pairs with another Pair on the same drum; a Follow follows a Lead or a
/// Pair; a Lead has no partner. `my_instrument < 0` means unknown (any drum), as the C++.
pub fn is_partner(my_role: i32, my_instrument: i32, their_role: i32, their_instrument: i32) -> bool {
    (my_role == ROLE_PAIR && their_role == ROLE_PAIR && (my_instrument < 0 || same_drum(my_instrument, their_instrument)))
        || (my_role == ROLE_FOLLOW && (their_role == ROLE_LEAD || their_role == ROLE_PAIR))
}

/// `collect`'s filter over one other instance's events: those on `clock` inside
/// `[from_pos, to_pos]`, a partner's triggers into `partners`, a neighbour's beacons into
/// `neighbours`. Never grows either buffer past its capacity (the audio thread's rule): what does
/// not fit is dropped, as the C++ drops what its 64-event ring no longer holds.
#[allow(clippy::too_many_arguments)]
pub fn collect_from(
    events: &[LinkEvent],
    partner: bool,
    clock: i32,
    from_pos: i64,
    to_pos: i64,
    partners: &mut Vec<LinkEvent>,
    neighbours: &mut Vec<LinkEvent>,
) {
    for e in events {
        if e.clock != clock || e.pos < from_pos || e.pos > to_pos {
            continue;
        }
        let out = if partner {
            if e.kind != EV_TRIGGER {
                continue;
            }
            &mut *partners
        } else {
            if e.kind != EV_BEACON {
                continue;
            }
            &mut *neighbours
        };
        if out.len() < out.capacity() {
            out.push(*e);
        }
    }
}

// ---------------------------------------------------------------- the slab layout

/// `maxInstances`.
pub const MAX_INSTANCES: usize = 16;
/// `eventsPerSlot`.
pub const EVENTS_PER_SLOT: usize = 64;
/// `AudioShare::ringSize`: 1.36 s at 48 kHz.
pub const RING_SIZE: usize = 65_536;

/// An event's words: `pos` (low, high), `levelDb`, `velocity`, `kind`, `clock`.
const EVENT_WORDS: usize = 6;
// `ShaleLink::Slot`'s header. `heartbeat` is `LIVE`; `OWNER` is this port's claim token.
const IN_USE: usize = 0;
const SEQ: usize = 1;
const LIVE: usize = 2;
const GROUP: usize = 3;
const ROLE: usize = 4;
const INSTRUMENT: usize = 5;
const FLAGS: usize = 6;
const WRITE_INDEX: usize = 7;
const OWNER: usize = 8;
const SLOT_HEADER: usize = 10;
/// One `ShaleLink::Slot`, in words.
pub const SLOT_WORDS: usize = SLOT_HEADER + EVENTS_PER_SLOT * EVENT_WORDS;
// `AudioShare::Slot`'s header: `seq`, `clock`, `sampleRate` (an f64, two words), `runStart` and
// `endPos` (i64s, two words each), then the ring.
const A_SEQ: usize = 0;
const A_CLOCK: usize = 1;
const A_RATE: usize = 2;
const A_RUN: usize = 4;
const A_END: usize = 6;
const A_HEADER: usize = 8;
/// One `AudioShare::Slot`, in words.
pub const AUDIO_SLOT_WORDS: usize = A_HEADER + RING_SIZE;
/// Where the audio region starts (after the 16 link slots).
const AUDIO_BASE: usize = MAX_INSTANCES * SLOT_WORDS;
/// The slab Shale needs: both regions, every slot (`sizeof (Region)` of both files, in words).
pub const WORDS: usize = AUDIO_BASE + MAX_INSTANCES * AUDIO_SLOT_WORDS;
/// The layout id: `kMagic` "SHLK".
pub const LAYOUT: u32 = 0x4B4C_4853;
/// `flagPhaseRef`.
pub const FLAG_PHASE_REF: i32 = 1;

/// Claim tokens: each claim gets its own, so a release can tell whether the slot is still its.
static NEXT_TOKEN: AtomicU32 = AtomicU32::new(1);

/// `Presence`: what a live instance publishes about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Presence {
    pub slot: usize,
    pub group: i32,
    pub role: i32,
    pub instrument: i32,
    pub phase_ref: bool,
    pub is_self: bool,
}

/// `class ShaleLink` plus this instance's side of `AudioShare`: its slot in its group's slab.
#[derive(Debug, Default)]
pub struct KitLink {
    slab: Option<Arc<LinkSlab>>,
    my_slot: Option<usize>,
    token: u32,
    /// `myWrite`: our own ring cursor.
    my_write: u32,
    /// `AudioShare::expectedNext`: the writer's own view of its run.
    expected_next: i64,
}

fn split(v: i64) -> (u32, u32) {
    (v as u64 as u32, ((v as u64) >> 32) as u32)
}

fn join(lo: u32, hi: u32) -> i64 {
    (u64::from(lo) | (u64::from(hi) << 32)) as i64
}

/// The ring index of a timeline position (`pos & (ringSize - 1)`, two's complement as the C++).
fn ring_at(pos: i64) -> usize {
    (pos & (RING_SIZE as i64 - 1)) as usize
}

/// A slab holding Shale's layout, big enough.
fn shale_region(slab: Option<&Arc<LinkSlab>>) -> Option<&LinkSlab> {
    slab.map(|s| &**s).filter(|s| s.layout() == LAYOUT && s.len() >= WORDS)
}

impl KitLink {
    /// The slab the host hands over (`None`: in no group) and this member's index: releases the
    /// old slot and claims the new one (`claimSlot`: the slot zeroed, in use). Atomic stores only:
    /// never allocates or blocks.
    pub fn set(&mut self, slab: Option<Arc<LinkSlab>>, member: usize) {
        self.leave();
        self.my_slot = (member < MAX_INSTANCES).then_some(member);
        self.slab = slab;
        self.my_write = 0;
        self.expected_next = -1;
        if let (Some(r), Some(s)) = (shale_region(self.slab.as_ref()), self.my_slot) {
            let at = s * SLOT_WORDS;
            for i in 0..SLOT_WORDS {
                r.store(at + i, 0);
            }
            self.token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed).max(1);
            r.store(at + OWNER, self.token);
            r.store(at + IN_USE, 1);
        }
    }

    /// `releaseSlot`: clears this instance's slot, if it still owns it.
    pub fn leave(&mut self) {
        if let (Some(r), Some(s)) = (self.region(), self.my_slot)
            && r.load(s * SLOT_WORDS + OWNER) == Some(self.token)
        {
            r.store(s * SLOT_WORDS + LIVE, 0);
            r.store(s * SLOT_WORDS + IN_USE, 0);
        }
    }

    /// `region()`: the slab when it holds Shale's layout and is big enough.
    fn region(&self) -> Option<&LinkSlab> {
        shale_region(self.slab.as_ref())
    }

    /// Whether the host has this instance in a link group.
    pub fn is_linked(&self) -> bool {
        self.slab.is_some()
    }

    /// `isUnavailable`: no usable slot (in no group, another plugin's layout in the group, or more
    /// than 16 members). Everything then degrades to a single-instance gate, as in the C++.
    pub fn is_unavailable(&self) -> bool {
        self.region().is_none() || self.my_slot.is_none()
    }

    /// `getSlot`.
    pub fn slot(&self) -> Option<usize> {
        self.my_slot
    }

    /// `publishPresence` (and `keepAlive`): this instance's group, role, Mic and flags; live.
    pub fn publish_presence(&self, group: i32, role: i32, instrument: i32, flags: i32) {
        let (Some(r), Some(s)) = (self.region(), self.my_slot) else { return };
        let at = s * SLOT_WORDS;
        r.store(at + GROUP, group as u32);
        r.store(at + ROLE, role as u32);
        r.store(at + INSTRUMENT, instrument as u32);
        r.store(at + FLAGS, flags as u32);
        r.store(at + LIVE, 1);
    }

    /// Kit Link switched off: no longer live (the C++'s presence going stale).
    pub fn withdraw(&self) {
        if let (Some(r), Some(s)) = (self.region(), self.my_slot) {
            r.store(s * SLOT_WORDS + LIVE, 0);
        }
    }

    /// `publishEvents`: appends this block's events to this instance's ring, under the seqlock.
    pub fn publish_events(&mut self, events: &[LinkEvent]) {
        let (Some(r), Some(s)) = (shale_region(self.slab.as_ref()), self.my_slot) else { return };
        if events.is_empty() {
            return;
        }
        let at = s * SLOT_WORDS;
        // Seqlock, writer side: odd, write, even again.
        let seq = r.load(at + SEQ).unwrap_or(0);
        r.store(at + SEQ, seq.wrapping_add(1));
        for e in events {
            let w = at + SLOT_HEADER + (self.my_write as usize & (EVENTS_PER_SLOT - 1)) * EVENT_WORDS;
            let (lo, hi) = split(e.pos);
            r.store(w, lo);
            r.store(w + 1, hi);
            r.store_f32(w + 2, e.level_db);
            r.store_f32(w + 3, e.velocity);
            r.store(w + 4, e.kind as u32);
            r.store(w + 5, e.clock as u32);
            self.my_write = self.my_write.wrapping_add(1);
        }
        r.store(at + WRITE_INDEX, self.my_write);
        r.store(at + SEQ, seq.wrapping_add(2));
    }

    /// The DAW's reset (Play, Stop, Locate): this instance's event ring starts empty and its shared
    /// audio starts a new run, so nothing published before the move is read against the new
    /// position. (A Rust addition: the C++ keeps both across `prepareToPlay`, where a stale event
    /// that happens to sit at the new position would be heard again.)
    pub fn restart(&mut self) {
        self.expected_next = -1;
        self.my_write = 0;
        let (Some(r), Some(s)) = (shale_region(self.slab.as_ref()), self.my_slot) else { return };
        let at = s * SLOT_WORDS;
        let seq = r.load(at + SEQ).unwrap_or(0);
        r.store(at + SEQ, seq.wrapping_add(1));
        r.store(at + WRITE_INDEX, 0);
        r.store(at + SEQ, seq.wrapping_add(2));
        let a = AUDIO_BASE + s * AUDIO_SLOT_WORDS;
        let aseq = r.load(a + A_SEQ).unwrap_or(0);
        r.store(a + A_SEQ, aseq.wrapping_add(1));
        for k in [A_RUN, A_RUN + 1, A_END, A_END + 1] {
            r.store(a + k, 0);
        }
        r.store(a + A_SEQ, aseq.wrapping_add(2));
    }

    /// Slot `i`'s presence while it is in use and live.
    pub fn presence(&self, i: usize) -> Option<Presence> {
        let r = self.region()?;
        if i >= MAX_INSTANCES {
            return None;
        }
        let at = i * SLOT_WORDS;
        if r.load(at + IN_USE)? == 0 || r.load(at + LIVE)? == 0 {
            return None;
        }
        Some(Presence {
            slot: i,
            group: r.load(at + GROUP)? as i32,
            role: r.load(at + ROLE)? as i32,
            instrument: r.load(at + INSTRUMENT)? as i32,
            phase_ref: (r.load(at + FLAGS)? as i32 & FLAG_PHASE_REF) != 0,
            is_self: Some(i) == self.my_slot,
        })
    }

    /// `collect`: everything the other live instances in `group` published inside
    /// `[from_pos, to_pos]` on `clock`, partners' triggers into `partners` and neighbours' beacons
    /// into `neighbours` (both cleared first; never grown past their capacity).
    #[allow(clippy::too_many_arguments)]
    pub fn collect(
        &self,
        group: i32,
        my_role: i32,
        clock: i32,
        from_pos: i64,
        to_pos: i64,
        partners: &mut Vec<LinkEvent>,
        neighbours: &mut Vec<LinkEvent>,
        my_instrument: i32,
    ) {
        partners.clear();
        neighbours.clear();
        let Some(r) = self.region() else { return };
        for i in 0..MAX_INSTANCES {
            if Some(i) == self.my_slot {
                continue;
            }
            let Some(p) = self.presence(i) else { continue };
            if p.group != group {
                continue;
            }
            let partner = is_partner(my_role, my_instrument, p.role, p.instrument);
            let mut local = [LinkEvent::default(); EVENTS_PER_SLOT];
            let mut count = 0usize;
            let at = i * SLOT_WORDS;
            for _ in 0..4 {
                let before = r.load(at + SEQ).unwrap_or(1);
                if before & 1 != 0 {
                    continue; // a write is in progress
                }
                let w = r.load(at + WRITE_INDEX).unwrap_or(0);
                count = (EVENTS_PER_SLOT as u32).min(w) as usize;
                let start = w.wrapping_sub(count as u32);
                for (q, e) in local.iter_mut().enumerate().take(count) {
                    let k = at + SLOT_HEADER + (start.wrapping_add(q as u32) as usize & (EVENTS_PER_SLOT - 1)) * EVENT_WORDS;
                    *e = LinkEvent {
                        pos: join(r.load(k).unwrap_or(0), r.load(k + 1).unwrap_or(0)),
                        level_db: r.load_f32(k + 2).unwrap_or(-120.0),
                        velocity: r.load_f32(k + 3).unwrap_or(0.0),
                        kind: r.load(k + 4).unwrap_or(0) as i32,
                        clock: r.load(k + 5).unwrap_or(0) as i32,
                    };
                }
                if r.load(at + SEQ) == Some(before) {
                    break; // clean read
                }
                count = 0;
            }
            collect_from(&local[..count], partner, clock, from_pos, to_pos, partners, neighbours);
        }
    }

    // ------------------------------------------------------------ the shared audio (AudioShare)

    /// `AudioShare::write`: this block's mono input at `block_start` on `clock`.
    pub fn audio_write(&mut self, mono: &[f32], block_start: i64, clock: i32, sample_rate: f64) {
        let (Some(r), Some(s)) = (shale_region(self.slab.as_ref()), self.my_slot) else { return };
        let n = mono.len();
        if n == 0 {
            return;
        }
        let a = AUDIO_BASE + s * AUDIO_SLOT_WORDS;
        let rate = f64::from_bits(u64::from(r.load(a + A_RATE).unwrap_or(0)) | (u64::from(r.load(a + A_RATE + 1).unwrap_or(0)) << 32));
        let new_run = block_start != self.expected_next || r.load(a + A_CLOCK).map(|c| c as i32) != Some(clock) || rate != sample_rate;
        // samples first, then the stamps that make them visible
        for (i, &x) in mono.iter().enumerate() {
            r.store_f32(a + A_HEADER + ring_at(block_start.wrapping_add(i as i64)), x);
        }
        let seq = r.load(a + A_SEQ).unwrap_or(0);
        r.store(a + A_SEQ, seq.wrapping_add(1));
        if new_run {
            let (lo, hi) = split(block_start);
            r.store(a + A_RUN, lo);
            r.store(a + A_RUN + 1, hi);
            r.store(a + A_CLOCK, clock as u32);
            let bits = sample_rate.to_bits();
            r.store(a + A_RATE, bits as u32);
            r.store(a + A_RATE + 1, (bits >> 32) as u32);
        }
        let end = block_start.wrapping_add(n as i64);
        let (lo, hi) = split(end);
        r.store(a + A_END, lo);
        r.store(a + A_END + 1, hi);
        r.store(a + A_SEQ, seq.wrapping_add(2));
        self.expected_next = end;
    }

    /// Slot `slot`'s stamps under its seqlock: `(base, clock, rate, runStart, endPos)`; `None`
    /// when four tries all met a write.
    fn audio_stamps(&self, slot: usize) -> Option<(usize, i32, f64, i64, i64)> {
        let r = self.region()?;
        if slot >= MAX_INSTANCES {
            return None;
        }
        let a = AUDIO_BASE + slot * AUDIO_SLOT_WORDS;
        for _ in 0..4 {
            let before = r.load(a + A_SEQ)?;
            let clock = r.load(a + A_CLOCK)? as i32;
            let rate = f64::from_bits(u64::from(r.load(a + A_RATE)?) | (u64::from(r.load(a + A_RATE + 1)?) << 32));
            let run = join(r.load(a + A_RUN)?, r.load(a + A_RUN + 1)?);
            let end = join(r.load(a + A_END)?, r.load(a + A_END + 1)?);
            if before & 1 == 0 && r.load(a + A_SEQ) == Some(before) {
                return Some((a, clock, rate, run, end));
            }
        }
        None
    }

    /// `AudioShare::newest`: the newest position `slot` has written on the transport clock at
    /// `sample_rate`, or -1 when it has nothing usable.
    pub fn audio_newest(&self, slot: usize, sample_rate: f64) -> i64 {
        match self.audio_stamps(slot) {
            Some((_, clock, rate, _, end)) if clock == CLK_TRANSPORT && rate == sample_rate => end,
            _ => -1,
        }
    }

    /// `AudioShare::read`: copies `[from, from + out.len())` of `slot`'s input into `out`. False
    /// if any of it is not there: not written yet, overwritten, from before a seek, the wrong clock
    /// or the wrong sample rate.
    pub fn audio_read(&self, slot: usize, from: i64, out: &mut [f32], sample_rate: f64) -> bool {
        let n = out.len();
        let Some(r) = self.region() else { return false };
        if n == 0 || n > RING_SIZE / 2 {
            return false;
        }
        let Some((a, clock, rate, run, end)) = self.audio_stamps(slot) else { return false };
        if clock != CLK_TRANSPORT || rate != sample_rate {
            return false;
        }
        // there, all of it, and in this run
        if from < run || from.saturating_add(n as i64) > end || from < end.saturating_sub((RING_SIZE - 8192) as i64) {
            return false;
        }
        for (i, o) in out.iter_mut().enumerate() {
            *o = r.load_f32(a + A_HEADER + ring_at(from.wrapping_add(i as i64))).unwrap_or(0.0);
        }
        // and not overwritten, or re-run, while we copied
        match self.audio_stamps(slot) {
            Some((_, _, _, run_after, end_after)) => run_after == run && from >= end_after.saturating_sub((RING_SIZE - 4096) as i64),
            None => false,
        }
    }
}

impl Drop for KitLink {
    fn drop(&mut self) {
        self.leave();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::fs::shale::shale_dsp::{I_FLOOR_TOM, I_KICK_OUT, I_OVERHEADS, I_RACK_TOM, I_ROOM};

    #[test]
    fn partners_are_pairs_on_one_drum_or_what_a_follow_follows() {
        assert!(same_drum(I_KICK_IN, I_SUB_KICK) && same_drum(I_SNARE_TOP, I_SNARE_BTM));
        assert!(!same_drum(I_KICK_OUT, I_SNARE_TOP) && !same_drum(I_RACK_TOM, I_FLOOR_TOM) && same_drum(I_ROOM, I_ROOM));
        // A snare pair does not pair with the kick pair in the same group.
        assert!(is_partner(ROLE_PAIR, I_SNARE_BTM, ROLE_PAIR, I_SNARE_TOP));
        assert!(!is_partner(ROLE_PAIR, I_SNARE_BTM, ROLE_PAIR, I_KICK_IN));
        assert!(!is_partner(ROLE_PAIR, I_SNARE_BTM, ROLE_LEAD, I_SNARE_TOP));
        assert!(is_partner(ROLE_PAIR, -1, ROLE_PAIR, I_KICK_IN));
        // A Follow follows a Lead or a Pair, whatever the drum; a Lead has no partner.
        assert!(is_partner(ROLE_FOLLOW, I_ROOM, ROLE_LEAD, I_SNARE_TOP) && is_partner(ROLE_FOLLOW, I_OVERHEADS, ROLE_PAIR, I_KICK_IN));
        assert!(!is_partner(ROLE_FOLLOW, I_ROOM, ROLE_FOLLOW, I_SNARE_TOP));
        assert!(!is_partner(ROLE_LEAD, I_SNARE_TOP, ROLE_PAIR, I_SNARE_TOP));
    }

    #[test]
    fn collect_sorts_by_kind_window_and_clock_without_growing() {
        let ev = |pos, kind, clock| LinkEvent { pos, kind, clock, ..LinkEvent::default() };
        let events =
            [ev(5, EV_TRIGGER, CLK_TRANSPORT), ev(6, EV_BEACON, CLK_TRANSPORT), ev(50, EV_TRIGGER, CLK_TRANSPORT), ev(7, EV_TRIGGER, CLK_WALL)];
        let (mut p, mut n) = (Vec::with_capacity(4), Vec::with_capacity(1));
        collect_from(&events, true, CLK_TRANSPORT, 0, 10, &mut p, &mut n);
        assert_eq!(p, [events[0]]);
        assert!(n.is_empty());
        collect_from(&events, false, CLK_TRANSPORT, 0, 100, &mut p, &mut n);
        assert_eq!(n, [events[1]]);
        collect_from(&events, false, CLK_TRANSPORT, 0, 100, &mut p, &mut n);
        assert_eq!(n.len(), 1, "a full buffer drops, never grows");
        assert_eq!(n.capacity(), 1);
    }
}
