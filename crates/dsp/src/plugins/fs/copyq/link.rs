//! CopyQ on a link group's value bus ([`LinkSlab`]): how instances share what they measure.
//!
//! One region per member (at most [`MAX_MEMBERS`]), written only by that member: a small header
//! and the member's measurement, all fifteen curves. A Send or Group member publishes its own
//! input as measured; a Match or Group member reads the others:
//!
//! - if the group has Send members, the target is their average (a Send is the leader: its
//!   audio passes untouched and everyone else comes to it);
//! - otherwise it is the average of the Group members, the reader included, so a group with no
//!   leader meets in the middle.
//!
//! A reader may see a curve half old and half new; both are measurements of the same sound a
//! moment apart, and the engine smooths the curve in time. Leaving (the host takes the slab away,
//! or the plugin is dropped) clears the region's role, only while this instance still owns it (a
//! claim token, as LevelUp's link has): a retired instance released late cannot clear a newcomer.
//! A member the host stops calling keeps its last measurement published.

use super::profile::{ALL, CURVES, FLOOR_DB, GRID_N, LV, Shape};
use crate::link::LinkSlab;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Members that can publish (later members only read).
pub const MAX_MEMBERS: usize = 16;
const HEADER: usize = 8;
/// Words per member.
pub const REGION: usize = HEADER + CURVES * GRID_N;
/// Words CopyQ needs in the slab.
pub const WORDS: usize = MAX_MEMBERS * REGION;
/// The layout id ("CPQ1").
pub const LAYOUT: u32 = 0x4350_5131;

const OWNER: usize = 0;
const ROLE: usize = 1;
const FLAGS: usize = 2;
const LEVEL: usize = 3;
const SECONDS: usize = 6;

/// A region's role word.
pub const ROLE_NONE: u32 = 0;
pub const ROLE_SEND: u32 = 1;
pub const ROLE_GROUP: u32 = 2;

const FLAG_STEREO: u32 = 1;
const FLAG_LOUD: u32 = 2;
const FLAG_QUIET: u32 = 4;

/// A member publishes once it has measured this much, seconds.
const MIN_SECONDS: f32 = 0.25;

static NEXT_TOKEN: AtomicU32 = AtomicU32::new(1);

/// This instance's place in its group's slab.
#[derive(Debug, Default)]
pub struct CopyQLink {
    slab: Option<Arc<LinkSlab>>,
    slot: Option<usize>,
    token: u32,
}

impl CopyQLink {
    /// The slab the host hands over (`None`: in no group) and this member's index: leaves the old
    /// region and claims the new one. Atomic stores only.
    pub fn set(&mut self, slab: Option<Arc<LinkSlab>>, member: usize) {
        self.leave();
        self.slot = (member < MAX_MEMBERS).then_some(member);
        self.slab = slab;
        self.token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed).max(1);
        if let (Some(slab), Some(at)) = (self.usable(), self.slot.map(|s| s * REGION)) {
            slab.store(at + ROLE, ROLE_NONE);
            slab.store(at + OWNER, self.token);
        }
    }

    /// The slab, when it is one CopyQ can read: every member agrees on the layout and it is long
    /// enough.
    fn usable(&self) -> Option<&LinkSlab> {
        self.slab.as_deref().filter(|s| s.layout() == LAYOUT && s.len() >= WORDS)
    }

    /// Whether the module is in a group it can use.
    pub fn linked(&self) -> bool {
        self.usable().is_some()
    }

    fn leave(&mut self) {
        if let (Some(slab), Some(at)) = (self.usable(), self.slot.map(|s| s * REGION))
            && slab.load(at + OWNER) == Some(self.token)
        {
            slab.store(at + ROLE, ROLE_NONE);
        }
    }

    /// Publishes `shape` (this module's input as measured over `seconds`) under `role`
    /// ([`ROLE_NONE`]: takes it down).
    pub fn publish(&self, role: u32, shape: &Shape, seconds: f32) {
        let (Some(slab), Some(at)) = (self.usable(), self.slot.map(|s| s * REGION)) else { return };
        if slab.load(at + OWNER) != Some(self.token) {
            return;
        }
        if role == ROLE_NONE || !shape.present || seconds < MIN_SECONDS {
            slab.store(at + ROLE, ROLE_NONE);
            return;
        }
        let has = |lv: usize| shape.has.get(lv).copied().unwrap_or(false);
        let flags = (u32::from(shape.stereo) * FLAG_STEREO) | (u32::from(has(super::profile::LOUD)) * FLAG_LOUD) | (u32::from(has(super::profile::QUIET)) * FLAG_QUIET);
        slab.store(at + FLAGS, flags);
        for (lv, level) in shape.level.iter().enumerate() {
            slab.store_f32(at + LEVEL + lv, *level);
        }
        slab.store_f32(at + SECONDS, seconds);
        let mut w = at + HEADER;
        for c in shape.curves.iter() {
            for v in c {
                slab.store_f32(w, *v);
                w += 1;
            }
        }
        slab.store(at + ROLE, role);
    }

    /// Reads the group's target into `out` (no allocation): the Send members' average, or with
    /// none of them and `peers` the Group members'. Returns how many Send and how many Group
    /// members publish; `out` is present only when it was made from at least one.
    pub fn gather(&self, peers: bool, out: &mut Shape) -> (u32, u32) {
        out.clear();
        let Some(slab) = self.usable() else { return (0, 0) };
        let members = (slab.members() as usize).min(MAX_MEMBERS);
        let role_of = |m: usize| slab.load(m * REGION + ROLE).unwrap_or(ROLE_NONE);
        let senders = (0..members).filter(|m| role_of(*m) == ROLE_SEND).count() as u32;
        let groupies = (0..members).filter(|m| role_of(*m) == ROLE_GROUP).count() as u32;
        let take = if senders > 0 {
            ROLE_SEND
        } else if peers && groupies > 0 {
            ROLE_GROUP
        } else {
            return (senders, groupies);
        };
        for c in out.curves.iter_mut() {
            c.fill(0.0);
        }
        out.level = [0.0; LV];
        let mut n = 0.0f32;
        let mut stereo = true;
        let mut has = [false; LV];
        for m in (0..members).filter(|m| role_of(*m) == take) {
            let at = m * REGION;
            let flags = slab.load(at + FLAGS).unwrap_or(0);
            let got = [true, flags & FLAG_LOUD != 0, flags & FLAG_QUIET != 0];
            stereo &= flags & FLAG_STEREO != 0;
            n += 1.0;
            for lv in 0..LV {
                // A member without a level stands in with all of its sound.
                let from = if got.get(lv).copied().unwrap_or(false) { lv } else { ALL };
                if let Some(h) = has.get_mut(lv) {
                    *h |= got.get(lv).copied().unwrap_or(false);
                }
                if let Some(l) = out.level.get_mut(lv) {
                    *l += slab.load_f32(at + LEVEL + from).filter(|v| v.is_finite()).unwrap_or(FLOOR_DB);
                }
                for ch in 0..super::profile::CH {
                    let src = at + HEADER + super::profile::curve_index(from, ch) * GRID_N;
                    if let Some(dst) = out.curves.get_mut(super::profile::curve_index(lv, ch)) {
                        for (i, d) in dst.iter_mut().enumerate() {
                            *d += slab.load_f32(src + i).filter(|v| v.is_finite()).map_or(FLOOR_DB, |v| v.clamp(FLOOR_DB, 40.0));
                        }
                    }
                }
            }
        }
        if n < 1.0 {
            out.clear();
            return (senders, groupies);
        }
        for c in out.curves.iter_mut() {
            for v in c.iter_mut() {
                *v /= n;
            }
        }
        for l in out.level.iter_mut() {
            *l /= n;
        }
        out.present = true;
        out.stereo = stereo;
        out.has = has;
        (senders, groupies)
    }
}

impl Drop for CopyQLink {
    fn drop(&mut self) {
        self.leave();
    }
}
