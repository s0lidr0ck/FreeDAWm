//! `LevelUpLink.{h,cpp}`: how an OUT instance finds its IN, on the host's link group slab
//! ([`LinkSlab`], B4) instead of the C++'s memory-mapped `pair.dat`.
//!
//! The C++ design, kept: one slot per instance (at most [`MAX_INSTANCES`] = 32), each instance
//! writes only its own slot (`inUse`, `role`, `group`, `lufs`), an IN publishes the short-term
//! loudness of its input, and an OUT takes the first live IN of the same pair (A-D, the `linkGroup`
//! parameter) whose loudness has been measured (`> -69`) as its target.
//!
//! What maps where:
//! - **The file** is the session's link group (`Insert::link_group`): the C++ file is one per
//!   machine, so every LevelUp anywhere shares it; here the modules the user puts in one group
//!   share a slab (on any track). The slab's [`LinkSlab::layout`] stands for the file's magic and
//!   version: a group that mixes LevelUp with another plugin is "from another version", and the
//!   pairing is unavailable, as in the C++.
//! - **A slot** is the region of this member's index ([`LinkSlab`] member indices are stable while
//!   the module stays in the group); an index past 31 has no slot ("all 32 pair slots are in use").
//! - **Claiming and leaving** (`claimSlot`, the destructor clearing `inUse`) happen when the host
//!   hands the plugin a slab or takes it away ([`crate::Plugin::set_link`]) and when the plugin is
//!   dropped. Leaving clears the slot only while this instance still owns it (a claim token in
//!   the slot, a word the C++ does not have), so a late release (a retired instance dropped after
//!   a newcomer took its member index) can't clear the newcomer's slot. That replaces the 4 s heartbeat: the host knows who is in the group, so a member that
//!   went away is cleared when it goes, not four seconds later.
//! - **The 10 Hz message-thread timer** that publishes and follows is the block: the IN publishes
//!   after each block, the OUT reads before each. The host renders a group's producers (IN) before
//!   its consumers (OUT) in a block, so the OUT sees its IN's latest value; the engine only reads
//!   the target every 100 ms of audio (`updateBlock`), so nothing is gained by the C++'s timer.
//!   Departure: a host that does not call `process` (a module bypassed in the strip) does not
//!   refresh its slot; its last value stays, as a C++ IN whose DSP is bypassed keeps publishing its
//!   last `shortTermIn`.

use crate::link::LinkSlab;

/// `maxInstances`.
pub const MAX_INSTANCES: usize = 32;
/// Words per slot: `inUse`, `role`, `group`, `lufs`, and this port's claim token.
pub const SLOT_WORDS: usize = 5;
/// The slab LevelUp needs (`sizeof (Region)`, in words).
pub const WORDS: usize = MAX_INSTANCES * SLOT_WORDS;
/// The layout id: `kMagic` ("LUVL") with `kVersion` 1 in the top byte's place.
pub const LAYOUT: u32 = 0x4C56_554C;

const IN_USE: usize = 0;
const ROLE: usize = 1;
const GROUP: usize = 2;
const LUFS: usize = 3;
const OWNER: usize = 4;

/// Claim tokens: each claim gets its own, so a release can tell whether the slot is still its.
static NEXT_TOKEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// `class LevelUpLink`: this instance's slot in its group's slab.
#[derive(Debug, Default)]
pub struct LevelUpLink {
    slab: Option<std::sync::Arc<LinkSlab>>,
    my_slot: Option<usize>,
    token: u32,
}

impl LevelUpLink {
    /// The slab the host hands over (`None`: in no group) and this member's index: leaves the old
    /// slot (the destructor's `inUse = 0`) and claims the new one (`claimSlot`: zeroed, in use,
    /// `lufs = -100`). Atomic stores only: never allocates or blocks.
    pub fn set(&mut self, slab: Option<std::sync::Arc<LinkSlab>>, member: usize) {
        self.leave();
        self.my_slot = (member < MAX_INSTANCES).then_some(member);
        self.slab = slab;
        let token = NEXT_TOKEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed).max(1);
        self.token = token;
        if let (Some(r), Some(s)) = (self.region(), self.my_slot) {
            let at = s * SLOT_WORDS;
            r.store(at + ROLE, 0);
            r.store(at + GROUP, 0);
            r.store_f32(at + LUFS, -100.0);
            r.store(at + OWNER, token);
            r.store(at + IN_USE, 1);
        }
    }

    /// Whether this instance still owns its slot (no newcomer has claimed it since).
    fn owns(&self, r: &LinkSlab, s: usize) -> bool {
        r.load(s * SLOT_WORDS + OWNER) == Some(self.token)
    }

    /// Clears this instance's slot (`r->slots[mySlot].inUse = 0`), if it still owns it.
    pub fn leave(&mut self) {
        if let (Some(r), Some(s)) = (self.region(), self.my_slot)
            && self.owns(r, s)
        {
            r.store(s * SLOT_WORDS + IN_USE, 0);
        }
    }

    /// `region()`: the slab when it holds LevelUp's layout and is big enough.
    fn region(&self) -> Option<&LinkSlab> {
        self.slab.as_deref().filter(|s| s.layout() == LAYOUT && s.len() >= WORDS)
    }

    /// `isUnavailable`: in a group, but without a usable slot (another plugin's layout in the
    /// group, or more than 32 members).
    pub fn is_unavailable(&self) -> bool {
        self.slab.is_some() && (self.region().is_none() || self.my_slot.is_none())
    }

    /// Whether the host has this instance in a group.
    pub fn is_linked(&self) -> bool {
        self.slab.is_some()
    }

    /// `publish (role, group, lufs)`.
    pub fn publish(&self, role: i32, group: i32, lufs: f32) {
        let (Some(r), Some(s)) = (self.region(), self.my_slot) else { return };
        if !self.owns(r, s) {
            return; // a newcomer has this slot now
        }
        let at = s * SLOT_WORDS;
        r.store(at + ROLE, role as u32);
        r.store(at + GROUP, group as u32);
        r.store_f32(at + LUFS, lufs);
        r.store(at + IN_USE, 1);
    }

    /// `findIn (group, lufsOut)`: the loudness of a live IN in this pair, other than this
    /// instance; `None` when there is none, or it has measured nothing yet.
    pub fn find_in(&self, group: i32) -> Option<f32> {
        let r = self.region()?;
        for i in 0..MAX_INSTANCES {
            if Some(i) == self.my_slot {
                continue;
            }
            let at = i * SLOT_WORDS;
            if r.load(at + IN_USE) != Some(1) {
                continue;
            }
            if r.load(at + GROUP).map(|g| g as i32) != Some(group) || r.load(at + ROLE) != Some(1) {
                continue; // 1 = IN
            }
            let lufs = r.load_f32(at + LUFS)?;
            if lufs <= -69.0 || lufs.is_nan() {
                continue; // nothing measured yet
            }
            return Some(lufs);
        }
        None
    }

    /// `countInGroup (group, role)`: live instances in this pair (this one included); `role < 0`:
    /// any role.
    pub fn count_in_group(&self, group: i32, role: i32) -> usize {
        let Some(r) = self.region() else { return 0 };
        (0..MAX_INSTANCES)
            .filter(|i| {
                let at = i * SLOT_WORDS;
                r.load(at + IN_USE) == Some(1)
                    && r.load(at + GROUP).map(|g| g as i32) == Some(group)
                    && (role < 0 || r.load(at + ROLE).map(|x| x as i32) == Some(role))
            })
            .count()
    }
}

impl Drop for LevelUpLink {
    fn drop(&mut self) {
        self.leave();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn slab() -> Arc<LinkSlab> {
        let s = Arc::new(LinkSlab::new(WORDS));
        s.set_members(1, 1, 0, LAYOUT);
        s
    }

    #[test]
    fn an_out_finds_the_in_of_its_pair() {
        let s = slab();
        let mut a = LevelUpLink::default();
        let mut b = LevelUpLink::default();
        a.set(Some(Arc::clone(&s)), 0);
        b.set(Some(Arc::clone(&s)), 1);
        // the IN has measured nothing yet
        a.publish(1, 0, -100.0);
        assert_eq!(b.find_in(0), None);
        a.publish(1, 0, -17.5);
        assert_eq!(b.find_in(0), Some(-17.5));
        // another pair, or not an IN
        assert_eq!(b.find_in(1), None);
        a.publish(2, 0, -17.5);
        assert_eq!(b.find_in(0), None);
        // never itself
        b.publish(1, 0, -12.0);
        assert_eq!(b.find_in(0), None);
        assert_eq!(a.find_in(0), Some(-12.0));
        assert_eq!((a.count_in_group(0, -1), a.count_in_group(0, 1)), (2, 1));
        // leaving clears the slot, and so does dropping
        a.publish(1, 0, -17.5);
        b.set(None, 0);
        assert!(!b.is_linked());
        assert_eq!(a.count_in_group(0, -1), 1);
        let mut c = LevelUpLink::default();
        c.set(Some(Arc::clone(&s)), 2);
        assert_eq!(c.find_in(0), Some(-17.5));
        drop(a);
        assert_eq!(c.find_in(0), None);
    }

    #[test]
    fn a_late_release_leaves_a_newcomers_slot_alone() {
        // A retired IN is dropped after a newcomer took its member index: its release (and any
        // publish it still makes) must not touch the newcomer's slot.
        let s = slab();
        let mut old = LevelUpLink::default();
        old.set(Some(Arc::clone(&s)), 0);
        old.publish(1, 0, -20.0);
        let mut new = LevelUpLink::default();
        new.set(Some(Arc::clone(&s)), 0);
        new.publish(1, 0, -14.0);
        let mut out = LevelUpLink::default();
        out.set(Some(Arc::clone(&s)), 1);
        assert_eq!(out.find_in(0), Some(-14.0));
        old.publish(2, 3, -30.0);
        assert_eq!(out.find_in(0), Some(-14.0), "a stale publish is ignored");
        drop(old);
        assert_eq!(out.find_in(0), Some(-14.0), "the newcomer is still in use");
        assert_eq!(out.count_in_group(0, 1), 1);
        drop(new);
        assert_eq!(out.find_in(0), None, "its owner's release clears it");
    }

    #[test]
    fn another_layout_or_too_many_members_is_unavailable() {
        let s = Arc::new(LinkSlab::new(WORDS));
        s.set_members(1, 1, 0, 0);
        let mut a = LevelUpLink::default();
        a.set(Some(Arc::clone(&s)), 0);
        assert!(a.is_unavailable());
        a.publish(1, 0, -10.0);
        assert_eq!(s.load(0), Some(0), "nothing written into a slab of another layout");
        let t = slab();
        let mut b = LevelUpLink::default();
        b.set(Some(Arc::clone(&t)), MAX_INSTANCES);
        assert!(b.is_unavailable());
        b.publish(1, 0, -10.0);
        assert!((0..WORDS).all(|i| t.load(i) == Some(0)));
        // a slab too short for 32 slots
        let short = Arc::new(LinkSlab::new(8));
        short.set_members(1, 0, 0, LAYOUT);
        let mut c = LevelUpLink::default();
        c.set(Some(short), 0);
        assert!(c.is_unavailable());
        assert_eq!(c.find_in(0), None);
        let free = LevelUpLink::default();
        assert!(!free.is_unavailable() && free.find_in(0).is_none());
    }
}
