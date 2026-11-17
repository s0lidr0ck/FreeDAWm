//! Built-in plugin instances and Clip Effects modules made off the audio thread.
//!
//! A realtime host turns on [`MixEngine::set_prepared_builtins`](crate::MixEngine::set_prepared_builtins)
//! and keeps a [`BuiltinPrep`] on its control thread. Before it hands a session to the audio
//! thread it calls [`BuiltinPrep::prepare`], which creates and prepares the built-in insert,
//! instrument and Clip Effects modules that session needs and the audio thread does not have yet
//! (the same way `sync` would: same constructor, same state, same channel count and block size),
//! and sends them along with the session. `sync` then seats them instead of creating them, as it
//! already did for hosted plug-ins ([`MixEngine::adopt`](crate::MixEngine::adopt)). What `sync`
//! replaces or removes is retired, never dropped on the audio thread.
//!
//! `BuiltinPrep` mirrors the decisions `sync` makes for a realtime engine (a slot is made anew
//! when its plugin or channel count changes; an External MIDI instrument also on a new state); if
//! the two ever disagree, `sync` still makes what is missing itself, so the sound never depends on
//! it (counted by [`MixEngine::made_in_sync`](crate::MixEngine::made_in_sync)).
//!
//! A new track's strip comes made the same way: its audio buffers, its delay-compensation and
//! key lines (reserved for a mix latency up to the engine's quarter-second headroom, with that
//! headroom on top) and its MIDI rings are sized here, as are a new bus's buffers;
//! when the engine's tables of strips, meters and busses would have to grow, larger empty ones
//! come along to move into. What the engine takes out of service goes back as
//! [`RetiredStrips`]. A strip `sync` had to make itself is counted by
//! [`MixEngine::strips_made_in_sync`](crate::MixEngine::strips_made_in_sync).

use crate::sidechain::{KeySlot, reads_bus};
use crate::{
    BusBuf, ClipFx, HEADROOM_S, MAX_CHANNELS, ModuleMeter, RELEASED_RESERVE, SlotKey, Strip, StripMeter, clip_fx_params, create_instance,
    external_midi, is_third_party, main_channels, reserved, strip_channels,
};
use soundcraft_dsp::Plugin;
use soundcraft_model::{BusId, Route, Session, Track, TrackId, TrackKind};
use std::collections::{HashMap, HashSet};

/// A new track's strip, made and sized off the audio thread ([`BuiltinPrep::prepare`]).
pub struct PreparedStrip {
    pub track: TrackId,
    pub(crate) strip: Strip,
}

impl PreparedStrip {
    /// Address and capacity of the strip's main buffers and delay lines, to check the engine
    /// renders with these very allocations.
    #[doc(hidden)]
    pub fn buffer_ids(&self) -> Vec<(usize, usize)> {
        self.strip.buffer_ids()
    }
}

/// Larger empty tables for the engine to move its strips, meters and busses into, so adding
/// tracks never grows a table on the audio thread.
pub struct StripRoom {
    pub(crate) strips: HashMap<TrackId, Strip>,
    pub(crate) meters: HashMap<TrackId, StripMeter>,
    pub(crate) busses: HashMap<BusId, BusBuf>,
}

/// Strips, tables and buffers the engine took out of service: drop them off the audio thread
/// ([`MixEngine::take_retired_strips`](crate::MixEngine::take_retired_strips)).
#[derive(Default)]
pub struct RetiredStrips {
    pub(crate) strips: Vec<Strip>,
    pub(crate) strip_maps: Vec<HashMap<TrackId, Strip>>,
    pub(crate) meter_maps: Vec<HashMap<TrackId, StripMeter>>,
    pub(crate) bus_maps: Vec<HashMap<BusId, BusBuf>>,
    pub(crate) buffers: Vec<Vec<Vec<f32>>>,
}

impl RetiredStrips {
    pub fn is_empty(&self) -> bool {
        self.strips.is_empty() && self.strip_maps.is_empty() && self.meter_maps.is_empty() && self.bus_maps.is_empty() && self.buffers.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.strips.clear();
        self.strip_maps.clear();
        self.meter_maps.clear();
        self.bus_maps.clear();
        self.buffers.clear();
    }
}

/// A strip for `t` with what `sync` and `reserve_delays` would size for it already sized
/// (allocates: control thread). `room` is the length the delay lines are reserved for.
fn sized_strip(s: &Session, t: &Track, ch: usize, mb: usize, room: usize) -> Strip {
    let mut st = Strip::new();
    st.buf = vec![vec![0.0; mb]; ch];
    st.pre = reserved(ch, mb);
    st.fader_gain = Vec::with_capacity(mb);
    st.mute_gains = Vec::with_capacity(mb);
    let inserts = &t.mixer.inserts;
    st.plugins.reserve(inserts.len());
    st.mods.resize(inserts.len(), ModuleMeter::default());
    st.analysis.reserve(inserts.len());
    st.released.reserve(RELEASED_RESERVE);
    let renders = !(t.inactive || matches!(t.kind, TrackKind::Master | TrackKind::Vca));
    for ins in inserts.iter().filter(|i| i.sidechain.is_some()) {
        let mut k = KeySlot::new(ins.id);
        k.buf = vec![vec![0.0; mb]; ch];
        if renders {
            k.delay.reserve(room, ch);
            k.main_delay.reserve(room, ch);
        }
        st.keys.push(k);
    }
    let sources = t.mixer.input.module_sources().len();
    let routed = s.tracks.iter().filter(|x| x.instrument.as_ref().and_then(crate::track_midi::target_of) == Some(t.id)).count();
    st.midi_src.reserve(sources + routed);
    st.routed_src.reserve(sources + routed);
    st.module_in.reserve(sources);
    st.inst_outs.sync(t, mb);
    let clips = t.clips().iter().filter(|c| c.is_audio() && clip_fx_params(c).is_some()).count();
    if clips > 0 {
        st.clip_fx.reserve(clips);
        st.clipbuf = vec![vec![0.0; mb]; ch];
    }
    let n = |on: bool| if on { ch } else { 0 };
    st.llm_dry = vec![vec![0.0; mb]; n(!inserts.is_empty())];
    st.bus_in = vec![vec![0.0; mb]; n(reads_bus(t))];
    st.print = vec![vec![0.0; mb]; n(t.kind == TrackKind::Audio && t.mixer.record_inserts)];
    if renders {
        st.align.reserve(room, ch);
        if t.mixer.delay > 0 {
            st.track_delay.reserve(t.mixer.delay as usize, ch);
        }
        st.inst_outs.reserve(|_| room);
        if t.mixer.sends.iter().flatten().any(|x| x.pre_fader) {
            st.align_pre.reserve(room, ch);
        }
        let aux = matches!(t.kind, TrackKind::Aux | TrackKind::Folder);
        if !aux && matches!(t.mixer.output, Route::Main | Route::Bus(_) | Route::Hardware(_)) {
            st.out_extra.reserve(room, ch);
            st.outbuf = reserved(ch, mb);
        }
    }
    st
}

/// Clip Effects modules made off the audio thread for one clip: only the modules the clip's
/// chain on the audio thread does not have yet.
pub struct PreparedClipFx {
    pub track: TrackId,
    /// The clip's id (`ClipId.0`).
    pub clip: u64,
    /// The channel count the modules are prepared for (the track's strip width).
    pub ch: usize,
    pub eq: Option<Box<dyn Plugin>>,
    pub comp: Option<Box<dyn Plugin>>,
}

/// Everything made off the audio thread for one session hand-over.
#[derive(Default)]
pub struct Prepared {
    /// Plugin instances (hosted and built-in), see [`crate::PreparedInstance`].
    pub instances: Vec<crate::PreparedInstance>,
    pub clip_fx: Vec<PreparedClipFx>,
    /// Strips of tracks the engine does not have yet.
    pub strips: Vec<PreparedStrip>,
    /// Buffers of busses the engine does not have yet.
    pub busses: Vec<(BusId, Vec<Vec<f32>>)>,
    /// Larger tables, when the engine's would have to grow.
    pub room: Option<StripRoom>,
}

impl Prepared {
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty() && self.clip_fx.is_empty() && self.strips.is_empty() && self.busses.is_empty() && self.room.is_none()
    }
}

impl From<Vec<crate::PreparedInstance>> for Prepared {
    fn from(instances: Vec<crate::PreparedInstance>) -> Self {
        Prepared { instances, ..Prepared::default() }
    }
}

/// What the control thread has handed to one realtime engine: per built-in slot (plugin id,
/// channels, the state load that makes it anew), per clip with effects (channels, EQ, compressor).
#[derive(Default)]
pub struct BuiltinPrep {
    slots: HashMap<(TrackId, SlotKey), (String, usize, u64)>,
    clips: HashMap<(TrackId, u64), (usize, bool, bool)>,
    /// Tracks whose strip was handed over, and busses whose buffers were (with the channels).
    tracks: HashSet<TrackId>,
    busses: HashMap<BusId, usize>,
    /// The capacity of the tables last handed over ([`StripRoom`]).
    room: usize,
    /// The sample rate (bits) the handed instances were prepared at.
    rate: u32,
}

impl BuiltinPrep {
    /// Forgets everything handed over: the next [`BuiltinPrep::prepare`] makes every module anew
    /// (the engine was replaced).
    pub fn clear(&mut self) {
        self.slots.clear();
        self.clips.clear();
        self.tracks.clear();
        self.busses.clear();
        self.room = 0;
    }

    /// Creates and prepares the built-in modules `s` needs that were not handed over yet
    /// (allocates: control thread). A change of sample rate starts afresh.
    pub fn prepare(&mut self, s: &Session, max_block: usize) -> Prepared {
        let sr = s.sample_rate.as_f64() as f32;
        // The engine clamps its block size the same way.
        let mb = max_block.clamp(16, 16_384);
        if sr.to_bits() != self.rate {
            self.clear();
            self.rate = sr.to_bits();
        }
        let main_ch = main_channels(s);
        let mut out = Prepared::default();
        let mut seen = HashSet::new();
        let mut seen_clips = HashSet::new();
        let room = ((HEADROOM_S * sr) as usize).saturating_mul(2);
        for t in &s.tracks {
            let ch = strip_channels(t, main_ch);
            if self.tracks.insert(t.id) {
                out.strips.push(PreparedStrip { track: t.id, strip: sized_strip(s, t, ch, mb, room) });
            }
            let inserts = t.mixer.inserts.iter().map(|i| (SlotKey::Module(i.id), i, ch, 0));
            // An External MIDI instrument is made anew on a new state (its port); see `sync`.
            let inst = t.instrument.iter().map(|i| (SlotKey::Instrument, i, ch.max(2), if i.plugin == external_midi::ID { i.state_load } else { 0 }));
            for (slot, ins, ch, load) in inserts.chain(inst) {
                if is_third_party(&ins.plugin) {
                    continue;
                }
                let key = (t.id, slot);
                seen.insert(key);
                let want = (ins.plugin.clone(), ch, load);
                if self.slots.get(&key) == Some(&want) {
                    continue;
                }
                let state = if ins.plugin == external_midi::ID { ins.state_bytes() } else { None };
                if let Some(n) = create_instance(&ins.plugin, state.as_deref(), sr, mb, ch) {
                    out.instances.push(crate::PreparedInstance { track: t.id, slot, id: ins.plugin.clone(), ch, plugin: n.plugin });
                }
                self.slots.insert(key, want);
            }
            for c in t.clips().iter().filter(|c| c.is_audio()) {
                let Some(params) = clip_fx_params(c) else { continue };
                let key = (t.id, c.id.0);
                seen_clips.insert(key);
                let (eq, comp) = ClipFx::needs(params);
                let have = match self.clips.get(&key) {
                    Some(&(hch, heq, hcomp)) if hch == ch => (heq, hcomp),
                    _ => (false, false),
                };
                if (eq && !have.0) || (comp && !have.1) {
                    let fx = ClipFx::new(sr, ch);
                    out.clip_fx.push(PreparedClipFx {
                        track: t.id,
                        clip: c.id.0,
                        ch,
                        eq: if eq && !have.0 { fx.make(ClipFx::EQ) } else { None },
                        comp: if comp && !have.1 { fx.make(ClipFx::COMP) } else { None },
                    });
                }
                self.clips.insert(key, (ch, eq, comp));
            }
        }
        self.slots.retain(|k, _| seen.contains(k));
        self.clips.retain(|k, _| seen_clips.contains(k));
        self.tracks.retain(|id| s.track(*id).is_some());
        for b in &s.busses {
            let n = b.format.channels().clamp(1, MAX_CHANNELS);
            if self.busses.insert(b.id, n) != Some(n) {
                out.busses.push((b.id, vec![vec![0.0; mb]; n]));
            }
        }
        self.busses.retain(|id, _| s.bus(*id).is_some());
        let n = s.tracks.len().max(s.busses.len());
        if n > self.room {
            let want = n.saturating_mul(2).max(16);
            let r = StripRoom { strips: HashMap::with_capacity(want), meters: HashMap::with_capacity(want), busses: HashMap::with_capacity(want) };
            self.room = r.strips.capacity().min(r.meters.capacity()).min(r.busses.capacity());
            out.room = Some(r);
        }
        out
    }
}
