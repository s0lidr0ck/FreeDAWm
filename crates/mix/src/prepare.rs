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

use crate::{ClipFx, SlotKey, clip_fx_params, create_instance, external_midi, is_third_party, main_channels, strip_channels};
use soundcraft_dsp::Plugin;
use soundcraft_model::{Session, TrackId};
use std::collections::{HashMap, HashSet};

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
}

impl Prepared {
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty() && self.clip_fx.is_empty()
    }
}

impl From<Vec<crate::PreparedInstance>> for Prepared {
    fn from(instances: Vec<crate::PreparedInstance>) -> Self {
        Prepared { instances, clip_fx: Vec::new() }
    }
}

/// What the control thread has handed to one realtime engine: per built-in slot (plugin id,
/// channels, the state load that makes it anew), per clip with effects (channels, EQ, compressor).
#[derive(Default)]
pub struct BuiltinPrep {
    slots: HashMap<(TrackId, SlotKey), (String, usize, u64)>,
    clips: HashMap<(TrackId, u64), (usize, bool, bool)>,
    /// The sample rate (bits) the handed instances were prepared at.
    rate: u32,
}

impl BuiltinPrep {
    /// Forgets everything handed over: the next [`BuiltinPrep::prepare`] makes every module anew
    /// (the engine was replaced).
    pub fn clear(&mut self) {
        self.slots.clear();
        self.clips.clear();
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
        for t in &s.tracks {
            let ch = strip_channels(t, main_ch);
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
        out
    }
}
