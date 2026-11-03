//! Instance link: a small value bus shared by the modules of one session-level link group
//! (`Insert::link_group`), for plugins that talk to their partners (FireStation LevelUp's IN/OUT
//! pairs, Shale's Kit Link).
//!
//! - **The slab.** An array of [`AtomicU32`]s (an `f32` travels as its bits) as long as the
//!   largest [`LinkSpec::words`] among the group's members (at most [`MAX_LINK_WORDS`]), made by
//!   the host when it syncs the session (off the steady-state audio path) and handed to every
//!   member through [`Plugin::set_link`](crate::Plugin::set_link) with the member's index. Reading
//!   and writing it never allocates, locks or blocks, from any thread. What each word means is the
//!   plugin's own business (a plugin with a region per member, Shale's audio ring, puts member
//!   `i`'s at `i × region`); members of one group are always instances of plugins that agree on it
//!   (the host links any modules the user puts in a group, so a plugin checks
//!   [`LinkSlab::layout`] before trusting the words).
//! - **Members.** Each member keeps its index (`0..`[`LinkSlab::members`]) for as long as it stays
//!   in the group; a leaver's index goes to the next newcomer. When the slab must grow (a member
//!   with a larger spec joins), the host makes a new one with the old words copied and hands it to
//!   every member; the old one is dropped off the audio thread.
//! - **Not automatable.** The role parameters a [`LinkSpec`] names set the rendering order, which
//!   changes only when the host re-syncs the session (an edit of the stored value), never from an
//!   automation lane: a plugin should not offer them for automation.
//! - **Order.** Within a block, the host renders a group's producers before its consumers (the
//!   same rule as sidechain sources and their consumers). A consumer the host can't order after a
//!   producer (a routing cycle, or a chain where it comes first) reads that producer's value from
//!   the previous block; the host reports it.
//! - **Partners.** The host keeps the group's member counts ([`LinkSlab::producers`] …) up to date
//!   at every sync. A consumer whose producer is gone keeps reading the last value written, so a
//!   plugin that must not act on a stale value checks the counts or a heartbeat of its own.

use std::sync::atomic::{AtomicU32, Ordering};

/// The most words a [`LinkSlab`] holds (8 MB): room for 16 members' 64k-sample rings and more.
pub const MAX_LINK_WORDS: usize = 1 << 21;

/// What a module does on its link group's value bus, which sets the rendering order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum LinkRole {
    /// Writes values its consumers read in the same block: renders before them.
    Producer,
    /// Reads its producers' values: renders after them.
    Consumer,
    /// Reads and writes on equal terms: no ordering among peers (a value may be a block old); the
    /// group's consumers render after its peers, as after its producers.
    Peer,
}

/// How a plugin takes part in instance links ([`PluginInfo::link`](crate::PluginInfo::link)):
/// which parameters say whether, and in what role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct LinkSpec {
    /// A switch parameter that turns linking on (`None`: always on while the module is in a
    /// group). Below 0.5 the module takes no part (no role).
    pub enable_param: Option<&'static str>,
    /// A choice parameter that selects the role (`None`: always `roles[0]`).
    pub role_param: Option<&'static str>,
    /// The role for each choice of `role_param` (its value rounded); `None`: no part. A value out
    /// of range takes no part.
    pub roles: &'static [Option<LinkRole>],
    /// The plugin's word layout, written by the host into [`LinkSlab::layout`] when every member
    /// of the group agrees on it (0 when they don't): a plugin reads the words only when it sees
    /// its own layout there.
    pub layout: u32,
    /// Words the plugin needs in the slab (all members together, headers included); the host
    /// makes the slab as long as the largest among the members (at most [`MAX_LINK_WORDS`]).
    pub words: usize,
}

impl LinkSpec {
    /// The role a module with these parameter values takes (`param` gives a parameter's value,
    /// `None` when it is not stored: then the default `fallback` gives it).
    pub fn role(&self, param: impl Fn(&str) -> Option<f32>, fallback: impl Fn(&str) -> f32) -> Option<LinkRole> {
        let value = |id: &str| param(id).filter(|v| v.is_finite()).unwrap_or_else(|| fallback(id));
        if let Some(on) = self.enable_param
            && value(on) < 0.5
        {
            return None;
        }
        let index = match self.role_param {
            None => 0,
            Some(p) => {
                let v = value(p);
                if !(v.is_finite() && v >= 0.0) {
                    return None;
                }
                v.round() as usize
            }
        };
        self.roles.get(index).copied().flatten()
    }
}

/// The role of a module running plugin `info` with stored parameters `params` (missing ones at
/// their defaults). `None`: the plugin does not link, or this module takes no part now.
pub fn link_role(info: &crate::PluginInfo, params: impl Fn(&str) -> Option<f32>) -> Option<LinkRole> {
    let spec = info.link.as_ref()?;
    spec.role(params, |id| info.param(id).map_or(0.0, |p| p.default))
}

/// One link group's value bus. See the module docs.
#[derive(Debug)]
pub struct LinkSlab {
    words: Box<[AtomicU32]>,
    producers: AtomicU32,
    consumers: AtomicU32,
    peers: AtomicU32,
    members: AtomicU32,
    layout: AtomicU32,
}

impl LinkSlab {
    /// A slab of `words` words (clamped to `1..=MAX_LINK_WORDS`), every one 0, and no members.
    /// Allocates: the host makes slabs when it syncs.
    pub fn new(words: usize) -> Self {
        let n = words.clamp(1, MAX_LINK_WORDS);
        LinkSlab {
            words: (0..n).map(|_| AtomicU32::new(0)).collect(),
            producers: AtomicU32::new(0),
            consumers: AtomicU32::new(0),
            peers: AtomicU32::new(0),
            members: AtomicU32::new(0),
            layout: AtomicU32::new(0),
        }
    }
    /// A slab of `words` words holding `old`'s words (as many as fit) and its member data.
    pub fn grown_from(old: &LinkSlab, words: usize) -> Self {
        let s = LinkSlab::new(words);
        for (d, o) in s.words.iter().zip(old.words.iter()) {
            d.store(o.load(Ordering::Acquire), Ordering::Release);
        }
        s.set_members(old.producers(), old.consumers(), old.peers(), old.layout());
        s.members.store(old.members(), Ordering::Release);
        s
    }
    /// The number of words.
    pub fn len(&self) -> usize {
        self.words.len()
    }
    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }
    /// Word `i` (`None` past the end).
    pub fn load(&self, i: usize) -> Option<u32> {
        self.words.get(i).map(|w| w.load(Ordering::Acquire))
    }
    /// Writes word `i`; `false` past the end.
    pub fn store(&self, i: usize, v: u32) -> bool {
        self.words.get(i).map(|w| w.store(v, Ordering::Release)).is_some()
    }
    /// Word `i` as an `f32` (`None` past the end).
    pub fn load_f32(&self, i: usize) -> Option<f32> {
        self.load(i).map(f32::from_bits)
    }
    /// Writes word `i` as an `f32`; `false` past the end.
    pub fn store_f32(&self, i: usize, v: f32) -> bool {
        self.store(i, v.to_bits())
    }
    /// Producers in the group as of the host's last sync (this instance included).
    pub fn producers(&self) -> u32 {
        self.producers.load(Ordering::Acquire)
    }
    /// Consumers in the group as of the host's last sync.
    pub fn consumers(&self) -> u32 {
        self.consumers.load(Ordering::Acquire)
    }
    /// Peers in the group as of the host's last sync.
    pub fn peers(&self) -> u32 {
        self.peers.load(Ordering::Acquire)
    }
    /// One more than the highest member index in use (members are numbered from 0; a gap is an
    /// index a leaver freed), as of the host's last sync.
    pub fn members(&self) -> u32 {
        self.members.load(Ordering::Acquire)
    }
    /// Host side: the member index count, set at every sync.
    pub fn set_member_count(&self, n: u32) {
        self.members.store(n, Ordering::Release);
    }
    /// The members' common [`LinkSpec::layout`] (0: the group mixes plugins that disagree).
    pub fn layout(&self) -> u32 {
        self.layout.load(Ordering::Acquire)
    }
    /// Host side: the group's member counts and common layout, set at every sync.
    pub fn set_members(&self, producers: u32, consumers: u32, peers: u32, layout: u32) {
        self.producers.store(producers, Ordering::Release);
        self.consumers.store(consumers, Ordering::Release);
        self.peers.store(peers, Ordering::Release);
        self.layout.store(layout, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn words_round_trip_and_out_of_range_is_refused() {
        let s = LinkSlab::new(256);
        assert_eq!(s.len(), 256);
        assert!(s.store_f32(3, -12.5));
        assert_eq!(s.load_f32(3), Some(-12.5));
        assert!(s.store(255, 7));
        assert_eq!(s.load(255), Some(7));
        assert!(!s.store(256, 1));
        assert_eq!(s.load(256), None);
        assert_eq!(s.load_f32(usize::MAX), None);
        s.set_members(1, 2, 0, 9);
        s.set_member_count(3);
        assert_eq!((s.producers(), s.consumers(), s.peers(), s.layout(), s.members()), (1, 2, 0, 9, 3));
        // grown: the words and member data carry over
        let g = LinkSlab::grown_from(&s, 70_000);
        assert_eq!((g.len(), g.load_f32(3), g.load(255), g.load(69_999)), (70_000, Some(-12.5), Some(7), Some(0)));
        assert_eq!((g.producers(), g.members()), (1, 3));
        // sizes are clamped
        assert_eq!(LinkSlab::new(0).len(), 1);
        assert_eq!(LinkSlab::new(usize::MAX).len(), MAX_LINK_WORDS);
    }

    #[test]
    fn roles_follow_the_parameters() {
        static SPEC: LinkSpec = LinkSpec {
            enable_param: Some("on"),
            role_param: Some("role"),
            roles: &[None, Some(LinkRole::Producer), Some(LinkRole::Consumer)],
            layout: 1,
            words: 4,
        };
        let role = |on: Option<f32>, r: Option<f32>| SPEC.role(|id| if id == "on" { on } else { r }, |_| 1.0);
        assert_eq!(role(Some(1.0), Some(1.0)), Some(LinkRole::Producer));
        assert_eq!(role(Some(1.0), Some(2.2)), Some(LinkRole::Consumer));
        assert_eq!(role(Some(1.0), Some(0.0)), None);
        assert_eq!(role(Some(0.0), Some(1.0)), None);
        // hostile values: out of range, negative, NaN (falls back to the default 1.0)
        assert_eq!(role(Some(1.0), Some(9.0)), None);
        assert_eq!(role(Some(1.0), Some(-3.0)), None);
        assert_eq!(role(Some(1.0), Some(f32::NAN)), Some(LinkRole::Producer));
        assert_eq!(role(None, Some(f32::INFINITY)), Some(LinkRole::Producer));
    }
}
