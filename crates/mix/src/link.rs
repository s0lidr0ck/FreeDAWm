//! Instance links: modules in one session-level link group (`Insert::link_group`) share a value
//! bus ([`LinkSlab`], see `soundcraft_dsp::link`), and a group's producers render before its
//! consumers within a block, through the same ordering as sidechains (`sidechain.rs`).
//!
//! - **Roles.** A module's role comes from its plugin's [`LinkSpec`](soundcraft_dsp::LinkSpec)
//!   and its stored parameters (LevelUp's Role: Off / IN / OUT), so a parameter edit can change
//!   the order (it is part of the structure fingerprint: the engine re-syncs).
//! - **Order.** Resolved in session order (tracks, then chain order), after the sidechains. A
//!   producer on the consumer's own track must come earlier in the chain. On another track, the
//!   producer's track renders in an earlier level (an edge like a sidechain's), unless that would
//!   close a loop ([`LinkProblem::Cycle`]), or the tracks render in phases that can't be ordered
//!   that way (a producer on an aux or a master fader feeding an audio track:
//!   [`LinkProblem::Unsupported`]). Such a consumer still gets the slab and reads the value its
//!   producer wrote one block earlier; the engine reports it, and `mix.module_link` refuses to
//!   make one.
//!   Peers write too (Shale's Lead and Pair publish triggers its Follows read): a consumer renders
//!   after its group's producers and peers alike where it can; peers among themselves take no
//!   order. A peer → consumer edge that can't be satisfied (a cycle, the chain order, the phases)
//!   is not a problem: the consumer reads that peer a block late, which peers tolerate by design
//!   (Shale reaches 12 ms back, as the C++ does across hosts' arbitrary order). So a Follow is
//!   ordered after every peer of the host group (not only those of its own Kit Link group A–D,
//!   which the mix engine does not see) but never refused or reported for it. Producer edges
//!   (hard) are all resolved before any peer edge (soft), so a Follow met earlier in session order
//!   can't take the order a producer → consumer pair needs.
//! - **Partners.** A module alone in its group (no consumer for a producer, no producer or peer
//!   for a consumer, no other peer or consumer for a peer) is [`LinkProblem::Alone`]: it works,
//!   with nothing to talk to. A
//!   consumer whose producer was deleted keeps the last value written (the plugin can tell from
//!   [`LinkSlab::producers`]).

use crate::sidechain::{is_aux, reaches};
use soundcraft_dsp::{LinkRole, LinkSlab};
use soundcraft_model::{ModuleId, Session, Track, TrackId, TrackKind};
use std::sync::Arc;

/// Why a linked module can't (fully) talk to its group in the same block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkProblem {
    /// Nobody to talk to: no consumer for a producer, no producer for a consumer, no other peer.
    Alone,
    /// The module's track is inactive (or a VCA fader), so it does not run.
    Inactive,
    /// A producer's track already depends on this module's track (busses, sidechains or other
    /// links): this consumer reads the value from the previous block.
    Cycle,
    /// A producer comes after this consumer in the same chain: it reads the previous block's value.
    Order,
    /// A combination the engine can't order (the reason says which); it reads the previous block.
    Unsupported(&'static str),
}

impl LinkProblem {
    /// Whether the module reads a value a block late (worth reporting), rather than having nobody
    /// to talk to.
    pub fn is_ordering(self) -> bool {
        matches!(self, LinkProblem::Cycle | LinkProblem::Order | LinkProblem::Unsupported(_))
    }
}

impl std::fmt::Display for LinkProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkProblem::Alone => f.write_str("no partner in the link group"),
            LinkProblem::Inactive => f.write_str("the track is inactive"),
            LinkProblem::Cycle => f.write_str("routing cycle: the producer's track already depends on this track; it reads the previous block"),
            LinkProblem::Order => f.write_str("the producer comes after it in the chain; it reads the previous block"),
            LinkProblem::Unsupported(why) => f.write_str(why),
        }
    }
}

/// A short status for a picker.
pub fn short_link_problem(p: LinkProblem) -> &'static str {
    match p {
        LinkProblem::Alone => "alone",
        LinkProblem::Inactive => "inactive",
        LinkProblem::Cycle => "cycle",
        LinkProblem::Order => "order",
        LinkProblem::Unsupported(_) => "unsupported",
    }
}

/// One linked module as the engine resolved it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkRoute {
    pub track: TrackId,
    pub module: ModuleId,
    pub group: u32,
    /// `None`: in the group but taking no part now (its plugin has no link, or it is switched off).
    pub role: Option<LinkRole>,
    /// `Ok`: talks to its group in the same block.
    pub result: Result<(), LinkProblem>,
}

/// One member, as resolution sees it.
struct Member {
    track: usize,
    index: usize,
    module: ModuleId,
    group: u32,
    role: Option<LinkRole>,
}

/// The role a module takes, from its plugin's spec and stored params.
pub(crate) fn role_of(ins: &soundcraft_model::Insert) -> Option<LinkRole> {
    let info = soundcraft_dsp::plugin_info(&ins.plugin)?;
    soundcraft_dsp::link_role(info, |id| ins.params.get(id).copied())
}

/// The rendering phase of a track: 0 audio/instrument/MIDI (levels), 1 aux/folder, 2 master.
fn phase(t: &Track) -> u8 {
    match t.kind {
        TrackKind::Master => 2,
        _ if is_aux(t) => 1,
        _ => 0,
    }
}

fn runs(t: &Track) -> bool {
    !t.inactive && t.kind != TrackKind::Vca
}

/// Resolves every link of `s` (with module `over`'s group replaced, for "what if" checks) and adds
/// the producer → consumer edges it accepts to `adj` (track index → tracks that render after it).
pub(crate) fn resolve(s: &Session, adj: &mut [Vec<usize>], over: Option<(TrackId, ModuleId, Option<u32>)>) -> Vec<LinkRoute> {
    let mut members = Vec::new();
    for (ti, t) in s.tracks.iter().enumerate() {
        for (index, ins) in t.mixer.inserts.iter().enumerate() {
            let group = match over {
                Some((ot, om, g)) if ot == t.id && om == ins.id => g,
                _ => ins.link_group,
            };
            let Some(group) = group else { continue };
            members.push(Member { track: ti, index, module: ins.id, group, role: role_of(ins) });
        }
    }
    if members.is_empty() {
        return Vec::new();
    }
    let track = |i: usize| s.tracks.get(i);
    let live = |m: &Member| track(m.track).is_some_and(runs);
    let mut routes = Vec::with_capacity(members.len());
    for m in &members {
        let Some(t) = track(m.track) else { continue };
        let result = if !runs(t) {
            Err(LinkProblem::Inactive)
        } else {
            let others = || members.iter().filter(|o| o.group == m.group && !(o.track == m.track && o.module == m.module) && live(o));
            match m.role {
                None => Ok(()),
                Some(LinkRole::Peer) => {
                    // another peer, or a consumer reading what the peers write
                    if others().any(|o| matches!(o.role, Some(LinkRole::Peer | LinkRole::Consumer))) { Ok(()) } else { Err(LinkProblem::Alone) }
                }
                Some(LinkRole::Producer) => {
                    if others().any(|o| o.role == Some(LinkRole::Consumer)) {
                        Ok(())
                    } else {
                        Err(LinkProblem::Alone)
                    }
                }
                Some(LinkRole::Consumer) => {
                    // Only the producer edges here (hard: a consumer that can't render after its
                    // producer reads a block late). The peer edges come in a second pass below,
                    // once every producer edge of the session is in, so a peer edge met earlier in
                    // session order can't close a loop that refuses a producer edge.
                    let mut result = Err(LinkProblem::Alone);
                    for p in others().filter(|o| o.role == Some(LinkRole::Producer)) {
                        let edge = order(s, adj, p, m);
                        // the first producer decides, unless a later one reads late
                        if result.is_err_and(|e| e == LinkProblem::Alone) || (result.is_ok() && edge.is_err()) {
                            result = edge;
                        }
                    }
                    result
                }
            }
        };
        routes.push(LinkRoute { track: t.id, module: m.module, group: m.group, role: m.role, result });
    }
    // Peers write too: a consumer also renders after its group's peers, where that can still be
    // ordered. A peer edge that can't be satisfied is tolerated (soft): what peers write is read
    // with a reach-back (Shale's 12 ms), so a block late still works. A peer gives a consumer with
    // no producer a partner.
    for m in members.iter().filter(|m| m.role == Some(LinkRole::Consumer) && live(m)) {
        let Some(id) = track(m.track).map(|t| t.id) else { continue };
        let Some(route) = routes.iter_mut().find(|r| r.track == id && r.module == m.module) else { continue };
        for p in members.iter().filter(|o| o.group == m.group && o.role == Some(LinkRole::Peer) && live(o)) {
            let edge = order(s, adj, p, m);
            if (edge.is_ok() || edge.is_err_and(LinkProblem::is_ordering)) && route.result == Err(LinkProblem::Alone) {
                route.result = Ok(());
            }
        }
    }
    routes
}

/// Whether producer `p` can render before consumer `c` in a block; adds the track edge it needs.
fn order(s: &Session, adj: &mut [Vec<usize>], p: &Member, c: &Member) -> Result<(), LinkProblem> {
    let (Some(pt), Some(ct)) = (s.tracks.get(p.track), s.tracks.get(c.track)) else { return Err(LinkProblem::Alone) };
    if p.track == c.track {
        return if p.index < c.index { Ok(()) } else { Err(LinkProblem::Order) };
    }
    match (phase(pt), phase(ct)) {
        // master faders run one after the other, in session order
        (2, 2) => {
            if p.track < c.track {
                Ok(())
            } else {
                Err(LinkProblem::Order)
            }
        }
        (2, _) => Err(LinkProblem::Unsupported("a producer on a master fader runs after every track; the consumer reads the previous block")),
        (_, 2) => Ok(()),
        (1, 0) => Err(LinkProblem::Unsupported(
            "a producer on an aux or folder track runs after audio, instrument and MIDI tracks; the consumer reads the previous block",
        )),
        _ => {
            if reaches(adj, c.track, p.track) {
                return Err(LinkProblem::Cycle);
            }
            if let Some(a) = adj.get_mut(p.track)
                && !a.contains(&c.track)
            {
                a.push(c.track);
            }
            Ok(())
        }
    }
}

/// Every link of the session, resolved as the mix engine renders them (session order).
pub fn link_routes(s: &Session) -> Vec<LinkRoute> {
    crate::sidechain::resolve(s).links
}

/// [`link_routes`] with module `module` of `track` in group `group` (`None`: unlinked).
pub fn link_routes_with(s: &Session, track: TrackId, module: ModuleId, group: Option<u32>) -> Vec<LinkRoute> {
    crate::sidechain::resolve_link_with(s, track, module, group).links
}

/// Routes of `after` that read a block late (an ordering problem) and did not in `before` for the
/// same reason: each needs reporting.
pub fn link_regressions(before: &[LinkRoute], after: &[LinkRoute]) -> Vec<LinkRoute> {
    after
        .iter()
        .filter(|a| a.result.is_err_and(LinkProblem::is_ordering))
        .filter(|a| !before.iter().any(|b| b.track == a.track && b.module == a.module && b.group == a.group && b.result == a.result))
        .copied()
        .collect()
}

/// Why putting module `module` of `track` into `group` must be refused: it would read a block
/// late itself (a cycle, a chain order or phase the engine can't satisfy), or another link would
/// start to. `None`: allowed (leaving a group always is).
pub fn link_refusal(s: &Session, track: TrackId, module: ModuleId, group: Option<u32>) -> Option<String> {
    group?;
    let before = link_routes(s);
    let after = link_routes_with(s, track, module, group);
    if let Some(Err(p)) = after.iter().find(|r| r.track == track && r.module == module).map(|r| r.result)
        && p.is_ordering()
    {
        return Some(p.to_string());
    }
    let broken: Vec<String> =
        link_regressions(&before, &after).iter().filter(|r| !(r.track == track && r.module == module)).map(|r| describe_link(s, r)).collect();
    (!broken.is_empty()).then(|| format!("it would make another linked module read a block late ({})", broken.join("; ")))
}

/// One line for a linked module: which, which group, and its problem.
pub fn describe_link(s: &Session, r: &LinkRoute) -> String {
    let name = s.track(r.track).map_or_else(|| format!("track {}", r.track.0), |t| t.name.clone());
    let module = s.track(r.track).and_then(|t| t.mixer.module(r.module)).map_or("module", |i| i.plugin.as_str());
    match r.result {
        Ok(()) => format!("{name} · {module}: link group {}", r.group),
        Err(p) => format!("{name} · {module}: link group {}: {p}", r.group),
    }
}

/// One link group as the mix engine keeps it: its slab and who holds which member index.
pub(crate) struct LinkGroup {
    pub group: u32,
    pub slab: Arc<LinkSlab>,
    /// Member index -> the module holding it (`None`: freed by a leaver, for the next newcomer).
    pub members: Vec<Option<(TrackId, ModuleId)>>,
}

impl LinkGroup {
    /// The member index of module `module` on `track`.
    pub fn member_of(&self, track: TrackId, module: ModuleId) -> Option<usize> {
        self.members.iter().position(|m| *m == Some((track, module)))
    }
}

/// The words a module's plugin asks for (`LinkSpec::words`; 0 for one that does not link).
fn words_of(ins: &soundcraft_model::Insert) -> usize {
    soundcraft_dsp::plugin_info(&ins.plugin).and_then(|i| i.link.as_ref()).map_or(0, |l| l.words)
}

/// The word layout a module's plugin uses (`LinkSpec::layout`; 0 for one that does not link).
fn layout_of(ins: &soundcraft_model::Insert) -> u32 {
    soundcraft_dsp::plugin_info(&ins.plugin).and_then(|i| i.link.as_ref()).map_or(0, |l| l.layout)
}

/// Brings the link groups in line with the session: a group still in use keeps its slab (its
/// values) and its members their indices; leavers free theirs, newcomers take the lowest free
/// one; a slab too short for a newcomer's spec is replaced by a longer copy. Slabs replaced or no
/// longer used go to `retired` (the host drops them off the audio thread). Member counts and the
/// common layout are set. Allocates: part of `sync`.
pub(crate) fn sync_groups(s: &Session, routes: &[LinkRoute], old: Vec<LinkGroup>, retired: &mut Vec<Arc<LinkSlab>>) -> Vec<LinkGroup> {
    let mut old = old;
    let mut out: Vec<LinkGroup> = Vec::new();
    for t in &s.tracks {
        for ins in &t.mixer.inserts {
            let Some(g) = ins.link_group else { continue };
            if out.iter().any(|x| x.group == g) {
                continue;
            }
            let now: Vec<(TrackId, ModuleId)> =
                s.tracks.iter().flat_map(|t| t.mixer.inserts.iter().filter(move |i| i.link_group == Some(g)).map(move |i| (t.id, i.id))).collect();
            let words = s.tracks.iter().flat_map(|t| t.mixer.inserts.iter()).filter(|i| i.link_group == Some(g)).map(words_of).max().unwrap_or(0);
            let mut grp = match old.iter().position(|x| x.group == g) {
                Some(i) => old.swap_remove(i),
                None => LinkGroup { group: g, slab: Arc::new(LinkSlab::new(words)), members: Vec::new() },
            };
            for m in &mut grp.members {
                if m.is_some_and(|x| !now.contains(&x)) {
                    *m = None;
                }
            }
            for id in now {
                if grp.member_of(id.0, id.1).is_some() {
                    continue;
                }
                match grp.members.iter().position(Option::is_none) {
                    Some(free) => {
                        if let Some(m) = grp.members.get_mut(free) {
                            *m = Some(id);
                        }
                    }
                    None => grp.members.push(Some(id)),
                }
            }
            while grp.members.last().is_some_and(Option::is_none) {
                grp.members.pop();
            }
            let want = words.clamp(1, soundcraft_dsp::MAX_LINK_WORDS);
            // A new group's slab is made above at the largest member's words. An existing slab
            // grows to that size only while every member (a non-linking plugin counts as layout 0)
            // reports the same layout: in a mixed group no member trusts the words, so a bigger
            // slab would be allocated for nothing. It never shrinks. So a group that turns uniform
            // again (the odd member left) grows on that sync if its plugin needs more words.
            let mut layouts = s.tracks.iter().flat_map(|t| t.mixer.inserts.iter()).filter(|i| i.link_group == Some(g)).map(layout_of);
            let first = layouts.next();
            let agree = layouts.all(|l| Some(l) == first);
            if agree && grp.slab.len() < want {
                let grown = Arc::new(LinkSlab::grown_from(&grp.slab, want));
                retired.push(std::mem::replace(&mut grp.slab, grown));
            }
            out.push(grp);
        }
    }
    retired.extend(old.into_iter().map(|g| g.slab));
    for grp in &out {
        let g = grp.group;
        let live = routes.iter().filter(|r| r.group == g && r.result != Err(LinkProblem::Inactive));
        let count = |role: LinkRole| u32::try_from(live.clone().filter(|r| r.role == Some(role)).count()).unwrap_or(u32::MAX);
        let mut layout: Option<u32> = None;
        let mut agree = true;
        for ins in s.tracks.iter().flat_map(|t| t.mixer.inserts.iter()).filter(|i| i.link_group == Some(g)) {
            let l = soundcraft_dsp::plugin_info(&ins.plugin).and_then(|i| i.link.as_ref()).map_or(0, |l| l.layout);
            match layout {
                None => layout = Some(l),
                Some(x) if x != l => agree = false,
                Some(_) => {}
            }
        }
        let layout = if agree { layout.unwrap_or(0) } else { 0 };
        grp.slab.set_members(count(LinkRole::Producer), count(LinkRole::Consumer), count(LinkRole::Peer), layout);
        grp.slab.set_member_count(u32::try_from(grp.members.len()).unwrap_or(u32::MAX));
    }
    out
}
