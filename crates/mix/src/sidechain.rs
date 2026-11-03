//! Module sidechain inputs: a chain module keyed by another track's signal
//! ([`Insert::sidechain`](soundcraft_model::Insert::sidechain)).
//!
//! - **Order.** A source renders before its consumer. Non-aux tracks render in levels: a track
//!   keyed by another non-aux track renders in a later level (each level in parallel). Auxes and
//!   folders render after every other track, in an order where bus producers and sidechain sources
//!   come first. So an aux can be keyed from any track, but a non-aux track cannot be keyed from an
//!   aux (that is reported as unsupported).
//! - **Cycles.** Sidechains are resolved in session order (tracks, then chain order). One whose
//!   source is the consumer's own track, or whose source already depends on the consumer through
//!   bus routing or earlier sidechains, would close a loop: it is refused with
//!   [`SidechainProblem::Cycle`] and its key is silent. `mix.module_sidechain` refuses to make one.
//! - **Key.** No source chosen: the plugin gets `None` and keys from its own input. A source that
//!   is missing (deleted), inactive, muted (including by solo or its VCA), master/VCA, or in a
//!   cycle gives a silent key (`Some` of zeros). The key has the consumer's channel count: channel
//!   `c` takes source channel `c mod n`, so a mono source feeds every channel; a mono consumer
//!   takes the average of a multichannel source.
//! - **Latency.** With delay compensation on, the key is aligned with the module's main input:
//!   whichever arrives earlier waits for the other (the main input's wait adds to the strip's
//!   latency, which delay compensation then absorbs), through [`DelayLine`]s with the usual 10 ms
//!   crossfade. Key buffers and lines are allocated in `sync`; rendering never allocates.

use crate::delay_line::DelayLine;
use soundcraft_model::{ModuleId, ModuleRef, Route, Session, SidechainSource, Track, TrackId, TrackKind};

/// Why a module's sidechain key is silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidechainProblem {
    /// The source track does not exist (it was deleted).
    Missing,
    /// The source track is inactive, so it renders nothing.
    Inactive,
    /// The source is the module's own track, or it already depends on that track through bus
    /// routing or other sidechains.
    Cycle,
    /// A combination the engine can't order (the reason says which).
    Unsupported(&'static str),
}

impl std::fmt::Display for SidechainProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SidechainProblem::Missing => f.write_str("the sidechain source track does not exist"),
            SidechainProblem::Inactive => f.write_str("the sidechain source track is inactive"),
            SidechainProblem::Cycle => f.write_str("routing cycle: the source already depends on this track (through busses or sidechains)"),
            SidechainProblem::Unsupported(why) => f.write_str(why),
        }
    }
}

/// One module's sidechain as the engine resolved it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SidechainRoute {
    pub track: TrackId,
    pub module: ModuleId,
    pub source: TrackId,
    pub pre_fader: bool,
    /// `Ok`: the key is live. `Err`: it is silent, for this reason.
    pub result: Result<(), SidechainProblem>,
}

/// Every sidechain of the session, resolved as the mix engine renders them (session order).
pub fn sidechain_routes(s: &Session) -> Vec<SidechainRoute> {
    resolve(s).routes
}

/// [`sidechain_routes`] as they would be with module `module` of `track` keyed from `source`
/// (`None`: no key), without changing the session.
pub fn sidechain_routes_with(s: &Session, track: TrackId, module: ModuleId, source: Option<SidechainSource>) -> Vec<SidechainRoute> {
    resolve_with(s, Over::Sidechain(track, module, source)).routes
}

/// Routes of `after` whose key is silent and was not silent for the same reason (and source)
/// in `before`: a sidechain an edit broke, or a broken one an edit added. Each needs reporting.
pub fn sidechain_regressions(before: &[SidechainRoute], after: &[SidechainRoute]) -> Vec<SidechainRoute> {
    after
        .iter()
        .filter(|a| a.result.is_err())
        .filter(|a| !before.iter().any(|b| b.track == a.track && b.module == a.module && b.source == a.source && b.result == a.result))
        .copied()
        .collect()
}

/// Why keying module `module` of `track` from `want` must be refused, as the engine resolves
/// sidechains (in session order, so a new route can break an older one): its own route would be
/// a cycle or unsupported, or a key that works now would fall silent. An inactive source is
/// allowed (the track may come back). `None`: allowed.
pub fn sidechain_refusal(s: &Session, track: TrackId, module: ModuleId, want: Option<SidechainSource>) -> Option<String> {
    let before = sidechain_routes(s);
    let after = sidechain_routes_with(s, track, module, want);
    if let Some(Err(p)) = after.iter().find(|r| r.track == track && r.module == module).map(|r| r.result)
        && p != SidechainProblem::Inactive
    {
        return Some(p.to_string());
    }
    let broken: Vec<String> =
        sidechain_regressions(&before, &after).iter().filter(|r| !(r.track == track && r.module == module)).map(|r| describe_route(s, r)).collect();
    (!broken.is_empty()).then(|| format!("routing cycle: it would silence another sidechain ({})", broken.join("; ")))
}

/// One line for a silent key: which module, which source, and why.
pub fn describe_route(s: &Session, r: &SidechainRoute) -> String {
    let name = |t: TrackId| s.track(t).map_or_else(|| format!("track {}", t.0), |t| t.name.clone());
    let module = s.track(r.track).and_then(|t| t.mixer.module(r.module)).map_or("module", |i| i.plugin.as_str());
    match r.result {
        Ok(()) => format!("{} · {module}: keyed from {}", name(r.track), name(r.source)),
        Err(p) => format!("{} · {module}: sidechain from {} is silent: {p}", name(r.track), name(r.source)),
    }
}

/// A short status for a picker: `None` when the key is live.
pub fn short_problem(p: SidechainProblem) -> &'static str {
    match p {
        SidechainProblem::Missing => "missing",
        SidechainProblem::Inactive => "inactive",
        SidechainProblem::Cycle => "cycle",
        SidechainProblem::Unsupported(_) => "unsupported",
    }
}

/// One module of a track's MIDI input (`Route::Modules`), resolved: the source renders before
/// the track, or that module's events are silent for a reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModuleInputRoute {
    /// The instrument or MIDI track that plays the module's MIDI.
    pub track: TrackId,
    /// The source: the track the module sits on, and the module.
    pub source: TrackId,
    pub module: ModuleId,
    /// `Ok`: the events flow. `Err`: nothing plays, for this reason.
    pub result: Result<(), SidechainProblem>,
}

/// What resolving the session's sidechain routing gives the engine.
pub(crate) struct Plan {
    pub routes: Vec<SidechainRoute>,
    /// Every track whose input is a module's MIDI, resolved after the sidechains.
    pub midi: Vec<ModuleInputRoute>,
    /// Track index → the level its strip renders in (non-aux tracks), or its rank (auxes).
    pub level: Vec<u32>,
    /// Track indices in rendering order: non-aux tracks by level, then auxes by rank.
    pub order: Vec<usize>,
    /// Every linked module, resolved after the sidechains (`link.rs`).
    pub links: Vec<crate::link::LinkRoute>,
}

/// One module's routing replaced, for "what if" checks.
#[derive(Clone, Copy)]
enum Over<'a> {
    None,
    Sidechain(TrackId, ModuleId, Option<SidechainSource>),
    Link(TrackId, ModuleId, Option<u32>),
    /// Track `.0`'s input replaced by these modules' MIDI (empty: by no module).
    Input(TrackId, &'a [ModuleRef]),
}

impl Plan {
    /// The source of an accepted sidechain (track index and id), `None` when silent.
    pub fn source(&self, s: &Session, track: TrackId, module: ModuleId) -> Option<(usize, TrackId)> {
        let r = self.routes.iter().find(|r| r.track == track && r.module == module)?;
        r.result.ok()?;
        Some((s.tracks.iter().position(|t| t.id == r.source)?, r.source))
    }

    /// Whether module `m` plays into `track` (its route was accepted).
    pub fn midi_ok(&self, track: TrackId, m: ModuleRef) -> bool {
        self.midi.iter().any(|r| r.track == track && r.source == m.track && r.module == m.module && r.result.is_ok())
    }
}

pub(crate) fn is_aux(t: &Track) -> bool {
    matches!(t.kind, TrackKind::Aux | TrackKind::Folder)
}

/// Strips `render` runs through `process_strip`.
fn renders(t: &Track) -> bool {
    !t.inactive && !matches!(t.kind, TrackKind::Master | TrackKind::Vca)
}

/// Whether `to` can be reached from `from` along `adj`.
pub(crate) fn reaches(adj: &[Vec<usize>], from: usize, to: usize) -> bool {
    let mut seen = vec![false; adj.len()];
    let mut stack = vec![from];
    while let Some(n) = stack.pop() {
        if n == to {
            return true;
        }
        if let Some(v) = seen.get_mut(n) {
            if *v {
                continue;
            }
            *v = true;
        }
        stack.extend(adj.get(n).into_iter().flatten().copied());
    }
    false
}

/// Longest-path rank of each node over `adj` restricted to nodes where `member` holds (a
/// topological rank when that subgraph is acyclic; on a cycle it stops after `n` rounds).
fn ranks(adj: &[Vec<usize>], member: &[bool]) -> Vec<u32> {
    let n = adj.len();
    let mut rank = vec![0u32; n];
    for _ in 0..n {
        let mut changed = false;
        for (u, outs) in adj.iter().enumerate() {
            if !member.get(u).copied().unwrap_or(false) {
                continue;
            }
            let ru = rank.get(u).copied().unwrap_or(0);
            for &v in outs {
                if member.get(v).copied().unwrap_or(false)
                    && let Some(rv) = rank.get_mut(v)
                    && *rv <= ru
                {
                    *rv = ru.saturating_add(1);
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
    rank
}

/// Resolves every sidechain and the rendering order. Allocates: call it from `sync`.
pub(crate) fn resolve(s: &Session) -> Plan {
    resolve_with(s, Over::None)
}

/// Every module MIDI input of the session, resolved as the mix engine renders them.
pub fn module_input_routes(s: &Session) -> Vec<ModuleInputRoute> {
    resolve(s).midi
}

/// Why making `input` the input of `track` must be refused, as the engine orders rendering: the
/// module would feed the track it sits on, or already depends on `track` through bus routing,
/// sidechains or other module inputs (a cycle), or the source is one the engine can't order
/// before the track (a module on an aux or folder); or a sidechain that works now would fall
/// silent. An inactive or missing source is allowed (it may come back). `None`: allowed.
pub fn module_input_refusal(s: &Session, track: TrackId, input: &Route) -> Option<String> {
    let after = resolve_with(s, Over::Input(track, input.module_sources()));
    if let Some(Err(p)) = after
        .midi
        .iter()
        .filter(|r| r.track == track)
        .map(|r| r.result)
        .find(|r| matches!(r, Err(p) if !matches!(p, SidechainProblem::Inactive | SidechainProblem::Missing)))
    {
        return Some(match p {
            SidechainProblem::Cycle => "That would feed a track into itself".to_string(),
            p => p.to_string(),
        });
    }
    let before = sidechain_routes(s);
    let broken: Vec<String> = sidechain_regressions(&before, &after.routes).iter().map(|r| describe_route(s, r)).collect();
    (!broken.is_empty()).then(|| format!("That would feed a track into itself: it would silence a sidechain ({})", broken.join("; ")))
}

/// [`resolve`] with one module's link group replaced.
pub(crate) fn resolve_link_with(s: &Session, track: TrackId, module: ModuleId, group: Option<u32>) -> Plan {
    resolve_with(s, Over::Link(track, module, group))
}

/// [`resolve`] with one module's sidechain or link group replaced (for "what if" checks).
fn resolve_with(s: &Session, over: Over<'_>) -> Plan {
    let n = s.tracks.len();
    // u → v: u must render before v. Bus routing first: a strip feeding a bus comes before the
    // auxes that read it.
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (u, tu) in s.tracks.iter().enumerate() {
        if !renders(tu) {
            continue;
        }
        let feeds = |b: &Route| tu.mixer.output == *b || tu.mixer.sends.iter().flatten().any(|x| x.target == *b);
        for (v, tv) in s.tracks.iter().enumerate() {
            if v != u
                && renders(tv)
                && is_aux(tv)
                && matches!(tv.mixer.input, Route::Bus(_))
                && feeds(&tv.mixer.input)
                && let Some(a) = adj.get_mut(u)
            {
                a.push(v);
            }
        }
    }
    let mut routes = Vec::new();
    for (ci, ct) in s.tracks.iter().enumerate() {
        for ins in &ct.mixer.inserts {
            let sc = match over {
                Over::Sidechain(t, m, sc) if t == ct.id && m == ins.id => sc,
                _ => ins.sidechain,
            };
            let Some(sc) = sc else { continue };
            let si = s.tracks.iter().position(|t| t.id == sc.track);
            let result = check(s, &adj, ci, si);
            if result.is_ok()
                && let Some(a) = si.and_then(|si| adj.get_mut(si))
            {
                a.push(ci);
            }
            routes.push(SidechainRoute { track: ct.id, module: ins.id, source: sc.track, pre_fader: sc.pre_fader, result });
        }
    }
    // Module MIDI inputs order after the sidechains: the source renders before the track that
    // plays its module's events.
    let mut midi = Vec::new();
    for (ci, ct) in s.tracks.iter().enumerate() {
        let refs = match over {
            Over::Input(t, r) if t == ct.id => r,
            _ => ct.mixer.input.module_sources(),
        };
        for &ModuleRef { track: source, module } in refs {
            let si = s.tracks.iter().position(|t| t.id == source);
            let result = check_midi(s, &adj, ci, si, module);
            if result.is_ok()
                && let Some(a) = si.and_then(|si| adj.get_mut(si))
                && !a.contains(&ci)
            {
                a.push(ci);
            }
            midi.push(ModuleInputRoute { track: ct.id, source, module, result });
        }
    }
    // Links order after sidechains: a link that would close a loop reads a block late instead.
    let link_over = match over {
        Over::Link(t, m, g) => Some((t, m, g)),
        _ => None,
    };
    let links = crate::link::resolve(s, &mut adj, link_over);
    let plain: Vec<bool> = s.tracks.iter().map(|t| !is_aux(t)).collect();
    let auxes: Vec<bool> = s.tracks.iter().map(is_aux).collect();
    let (rp, ra) = (ranks(&adj, &plain), ranks(&adj, &auxes));
    let level: Vec<u32> = (0..n).map(|i| if auxes.get(i).copied().unwrap_or(false) { ra.get(i) } else { rp.get(i) }.copied().unwrap_or(0)).collect();
    let mut order: Vec<usize> = (0..n).filter(|&i| plain.get(i).copied().unwrap_or(false)).collect();
    order.sort_by_key(|&i| level.get(i).copied().unwrap_or(0));
    let mut aux_order: Vec<usize> = (0..n).filter(|&i| auxes.get(i).copied().unwrap_or(false)).collect();
    aux_order.sort_by_key(|&i| level.get(i).copied().unwrap_or(0));
    order.extend(aux_order);
    Plan { routes, midi, level, order, links }
}

/// Whether track `ci` (an instrument or MIDI track) may play module `module` of track `si` given
/// the routing so far.
fn check_midi(s: &Session, adj: &[Vec<usize>], ci: usize, si: Option<usize>, module: ModuleId) -> Result<(), SidechainProblem> {
    let Some(si) = si else { return Err(SidechainProblem::Missing) };
    let (Some(ct), Some(st)) = (s.tracks.get(ci), s.tracks.get(si)) else { return Err(SidechainProblem::Missing) };
    if !ct.kind.is_midi() {
        return Err(SidechainProblem::Unsupported("only an instrument or MIDI track plays a module's MIDI"));
    }
    if st.mixer.module(module).is_none() {
        return Err(SidechainProblem::Missing);
    }
    if matches!(st.kind, TrackKind::Master | TrackKind::Vca) {
        return Err(SidechainProblem::Unsupported("a module on a master or VCA track can't be a MIDI source"));
    }
    if si == ci || reaches(adj, ci, si) {
        return Err(SidechainProblem::Cycle);
    }
    if is_aux(st) {
        return Err(SidechainProblem::Unsupported("a module on an aux or folder track can't feed an instrument track (auxes render after it)"));
    }
    if st.inactive {
        return Err(SidechainProblem::Inactive);
    }
    Ok(())
}

/// Whether consumer track `ci` may take a key from track `si` given the routing so far.
fn check(s: &Session, adj: &[Vec<usize>], ci: usize, si: Option<usize>) -> Result<(), SidechainProblem> {
    let Some(si) = si else { return Err(SidechainProblem::Missing) };
    let (Some(ct), Some(st)) = (s.tracks.get(ci), s.tracks.get(si)) else { return Err(SidechainProblem::Missing) };
    if matches!(ct.kind, TrackKind::Master | TrackKind::Vca) {
        return Err(SidechainProblem::Unsupported("modules on master and VCA tracks take no sidechain"));
    }
    if matches!(st.kind, TrackKind::Master | TrackKind::Vca) {
        return Err(SidechainProblem::Unsupported("a master or VCA track can't be a sidechain source"));
    }
    if si == ci || reaches(adj, ci, si) {
        return Err(SidechainProblem::Cycle);
    }
    if !is_aux(ct) && is_aux(st) {
        return Err(SidechainProblem::Unsupported(
            "an audio, instrument or MIDI track can't take a sidechain from an aux or folder track (auxes render after it)",
        ));
    }
    if st.inactive {
        return Err(SidechainProblem::Inactive);
    }
    Ok(())
}

/// The key of one chain module, owned by the consumer's strip (sized in `sync`).
pub(crate) struct KeySlot {
    pub module: ModuleId,
    /// The accepted source (track index, id); `None` gives a silent key.
    pub source: Option<(usize, TrackId)>,
    pub pre_fader: bool,
    /// One buffer per consumer channel, `max_block` long.
    pub buf: Vec<Vec<f32>>,
    /// Timeline delay of the key as copied this block (the source's latency so far).
    pub at: usize,
    /// Aligns the key with the main input when the key is earlier.
    pub delay: DelayLine,
    /// Aligns the main input with the key when the key is later.
    pub main_delay: DelayLine,
}

impl KeySlot {
    pub fn new(module: ModuleId) -> Self {
        KeySlot { module, source: None, pre_fader: false, buf: Vec::new(), at: 0, delay: DelayLine::default(), main_delay: DelayLine::default() }
    }
}

/// What a source strip offers a key this block: its post-fader (`buf`) and pre-fader (`pre`)
/// signals and their timeline delays, whether it is muted, and the block it was rendered in.
pub(crate) struct KeySource<'a> {
    pub buf: &'a [Vec<f32>],
    pub pre: &'a [Vec<f32>],
    pub post_at: usize,
    pub pre_at: usize,
    pub muted: bool,
    pub stamp: u64,
}

/// Copies this block's key from `src` (silence when absent, muted, or not rendered in `block`).
/// Never allocates.
pub(crate) fn fill_key(key: &mut KeySlot, src: Option<KeySource<'_>>, block: u64, frames: usize) {
    let src = src.filter(|x| x.stamp == block);
    if let Some(x) = &src {
        key.at = if key.pre_fader { x.pre_at } else { x.post_at };
    } else {
        key.at = 0;
    }
    let from: &[Vec<f32>] = match &src {
        Some(x) if !x.muted => {
            if key.pre_fader {
                x.pre
            } else {
                x.buf
            }
        }
        _ => &[],
    };
    let ns = from.len();
    let nc = key.buf.len();
    for (c, dst) in key.buf.iter_mut().enumerate() {
        let n = frames.min(dst.len());
        let Some(d) = dst.get_mut(..n) else { continue };
        d.fill(0.0);
        if ns == 0 {
            continue;
        }
        if nc == 1 && ns > 1 {
            let g = 1.0 / ns as f32;
            for ch in from {
                for (y, x) in d.iter_mut().zip(ch.iter()) {
                    *y += x * g;
                }
            }
        } else if let Some(ch) = from.get(c % ns) {
            for (y, x) in d.iter_mut().zip(ch.iter()) {
                *y = *x;
            }
        }
    }
}
