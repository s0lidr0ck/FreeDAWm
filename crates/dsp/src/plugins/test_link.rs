//! A probe for the host's instance links and reference slots, built only with the `test-plugins`
//! feature (tests and the offscreen `ui_shot` render; never the app). It is not in the menu
//! ([`crate::plugins()`]); `plugin_info`/`create` find it by id.
//!
//! - **Role** (Off / Producer / Consumer / Peer) is its link role.
//! - A producer writes the block's transport position (+ 1; 0 is "never written") to word 0 and
//!   its Value to word 1 of the group's slab. A consumer replaces its audio with 0 on channel 0
//!   when word 0 holds its own block's position (its producer ran earlier in the same block), 1
//!   otherwise (a block late, or nothing written), and with word 1 on channel 1. Without a slab a
//!   consumer puts −1000 on channel 0.
//! - With Role Off and a reference in slot 0, it replaces channel 0 with the reference's first
//!   channel at the transport position (`Transport::samples`), silence outside it. With Role Off
//!   and no reference it scales its input by Value through a one-pole smoother that glides from
//!   wherever it was and is seated at the target only by `reset` (as a typical plugin's
//!   smoothers), for the host's "seated at the automated value" tests.

use crate::link::{LinkRole, LinkSlab, LinkSpec};
use crate::{Category, ParamInfo, Plugin, PluginInfo, SampleSource, Taper, Transport, Unit};
use std::sync::Arc;

const ROLES: &[&str] = &["Off", "Producer", "Consumer", "Peer"];

static PARAMS: [ParamInfo; 2] = [
    ParamInfo {
        id: "role",
        name: "Role",
        min: 0.0,
        max: 3.0,
        default: 0.0,
        unit: Unit::None,
        taper: Taper::Linear,
        choices: ROLES,
        display: None,
        group: "",
    },
    ParamInfo {
        id: "value",
        name: "Value",
        min: 0.0,
        max: 1.0,
        default: 0.5,
        unit: Unit::None,
        taper: Taper::Linear,
        choices: &[],
        display: None,
        group: "",
    },
];

pub static TEST_LINK_INFO: PluginInfo = PluginInfo {
    id: "test_link",
    name: "Link Probe",
    short_name: "LnkProbe",
    category: Category::Other,
    params: &PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: Some(LinkSpec {
        enable_param: None,
        role_param: Some("role"),
        roles: &[None, Some(LinkRole::Producer), Some(LinkRole::Consumer), Some(LinkRole::Peer)],
        layout: 0x7E57,
        words: 256,
    }),
    references: 2,
    midi_out: false,
};

/// The same probe asking for a slab of 64k words more (a member region as Shale's audio ring
/// needs), to test that the host grows a group's slab.
pub static TEST_LINK_BIG_INFO: PluginInfo = PluginInfo {
    id: "test_link_big",
    name: "Link Probe (big)",
    short_name: "LnkBig",
    link: Some(LinkSpec {
        enable_param: None,
        role_param: Some("role"),
        roles: &[None, Some(LinkRole::Producer), Some(LinkRole::Consumer), Some(LinkRole::Peer)],
        layout: 0x7E57,
        words: 256 + 65_536,
    }),
    ..TEST_LINK_INFO
};

/// See the module docs.
pub struct TestLink {
    info: &'static PluginInfo,
    role: f32,
    value: f32,
    link: Option<Arc<LinkSlab>>,
    refs: [Option<Arc<dyn SampleSource>>; 2],
    pos: i64,
    /// This instance's member index in its link group.
    pub member: usize,
    /// The smoothed Value gain (Role Off).
    gain: f32,
}

impl Default for TestLink {
    fn default() -> Self {
        Self::new()
    }
}

impl TestLink {
    pub fn new() -> Self {
        Self::with_info(&TEST_LINK_INFO)
    }
    /// The probe as `test_link_big`.
    pub fn big() -> Self {
        Self::with_info(&TEST_LINK_BIG_INFO)
    }
    fn with_info(info: &'static PluginInfo) -> Self {
        TestLink { info, role: 0.0, value: 0.5, link: None, refs: [None, None], pos: 0, member: 0, gain: 0.5 }
    }
}

impl Plugin for TestLink {
    fn info(&self) -> &'static PluginInfo {
        self.info
    }
    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {}
    fn reset(&mut self) {
        self.gain = self.value;
    }
    fn set_param(&mut self, id: &str, value: f32) -> bool {
        let Some(p) = PARAMS.iter().find(|p| p.id == id) else { return false };
        let v = p.clamp(value);
        match id {
            "role" => self.role = v.round(),
            _ => self.value = v,
        }
        true
    }
    fn param(&self, id: &str) -> Option<f32> {
        match id {
            "role" => Some(self.role),
            "value" => Some(self.value),
            _ => None,
        }
    }
    fn set_transport(&mut self, transport: &Transport) {
        self.pos = transport.samples;
    }
    fn set_link(&mut self, link: Option<Arc<LinkSlab>>, member: usize) {
        // The host keeps its own reference to the slab this lets go of (dropped off the audio
        // thread), so a plain replace here never frees it.
        self.link = link;
        self.member = member;
    }
    fn set_asset(&mut self, key: &str, asset: Option<Arc<dyn SampleSource>>) -> bool {
        let Some(i) = crate::REFERENCE_KEYS.iter().position(|k| *k == key) else { return false };
        // Dropping the reference replaced here is fine for a probe: the host keeps its own `Arc`
        // of every asset it hands over (and retires it off the audio thread), so this is never
        // the last one. A real plugin that holds an asset past the host's swap (a crossfade)
        // gives it back through `take_released_asset` instead, as FireStation Amp does.
        match self.refs.get_mut(i) {
            Some(r) => {
                *r = asset;
                true
            }
            None => false,
        }
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let stamp = (self.pos as u32).wrapping_add(1);
        let fill = |io: &mut [Vec<f32>], c: usize, v: f32| {
            if let Some(ch) = io.get_mut(c) {
                ch.iter_mut().take(frames).for_each(|x| *x = v);
            }
        };
        // Any linked member marks its index (word 100 + member) so the host's numbering shows.
        if let Some(l) = &self.link {
            l.store(100usize.saturating_add(self.member), 1);
        }
        match self.role as u32 {
            1 => {
                if let Some(l) = &self.link {
                    l.store(0, stamp);
                    l.store_f32(1, self.value);
                }
            }
            2 => match &self.link {
                Some(l) => {
                    let late = l.load(0) != Some(stamp);
                    fill(io, 0, if late { 1.0 } else { 0.0 });
                    fill(io, 1, l.load_f32(1).unwrap_or(0.0));
                }
                None => fill(io, 0, -1000.0),
            },
            0 => {
                let Some(r) = self.refs.first().and_then(Option::as_ref) else {
                    for c in 0..io.len() {
                        let mut g = self.gain;
                        if let Some(ch) = io.get_mut(c) {
                            for x in ch.iter_mut().take(frames) {
                                g += 0.01 * (self.value - g);
                                *x *= g;
                            }
                        }
                        if c + 1 == io.len() {
                            self.gain = g;
                        }
                    }
                    return;
                };
                let Some(src) = r.channel(0) else { return };
                if let Some(ch) = io.get_mut(0) {
                    for (i, x) in ch.iter_mut().take(frames).enumerate() {
                        let at = self.pos.saturating_add(i as i64);
                        *x = usize::try_from(at).ok().and_then(|a| src.get(a)).copied().unwrap_or(0.0);
                    }
                }
            }
            _ => {}
        }
    }
}
