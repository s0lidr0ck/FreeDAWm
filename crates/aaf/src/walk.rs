//! From the AAF object model to the neutral [`Import`]: the top composition's sound slots become
//! tracks, their sequences clips, transitions fades and crossfades, gain and pan operation
//! groups clip gain and track gain/pan, event slots markers; every clip's source is followed
//! through master and source mobs to its essence.
//!
//! The AAF Edit Protocol's conventions are followed (a transition overlaps the segments either
//! side of it; operation groups wrap the segment they act on; a master mob slot refers to one
//! channel of a file source mob, named by the slot's physical track number), plus the vendor
//! conventions seen in Pro Tools, Media Composer, Premiere and Resolve files: Avid's "Audio Gain"
//! with `AvidControlClipRatio` clip-gain envelopes, the `AudioMixerCompMute`/`Solo` slot
//! attributes, the `Curve Type` of an audio dissolve, the "Audio Channel Combiner" that groups
//! mono sources into a stereo or surround clip, and Avid's comment-marker user name.

use crate::Error;
use crate::aaf::{MobId, Reader, Value, indirect, mob_id, pid};
use crate::desc::*;
use crate::object::Object;
use std::collections::HashMap;

/// How the file is read.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// The rate positions are converted to.
    pub sample_rate: u32,
    /// The largest embedded audio stream read (bytes).
    pub max_embedded: u64,
    /// Pro Tools exports each fade as a clip of rendered audio ("Fade ") between one-frame
    /// "sample accurate edit" clips: turn them back into fades and crossfades on the clips either
    /// side (where the clips' media reaches far enough).
    pub pt_true_fades: bool,
    /// Pro Tools' one-frame "sample accurate edit" clips merged into the clip beside them.
    pub pt_remove_sae: bool,
    /// `pt_true_fades` for files Pro Tools wrote (whatever the two fields above say).
    pub pt_auto: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { sample_rate: 48_000, max_embedded: 2 << 30, pt_true_fades: false, pt_remove_sae: false, pt_auto: false }
    }
}

/// Whether the writer named in the header is Pro Tools.
pub fn is_pro_tools(writer: &str) -> bool {
    let w = writer.to_ascii_lowercase().replace(' ', "");
    w.contains("protools") || w.contains("digidesign")
}

/// Deepest nesting followed (segments inside segments, mob to mob).
const MAX_DEPTH: usize = 24;
/// Most clips placed (a guard against absurd files).
const MAX_CLIPS: usize = 500_000;
/// Most control points read from one varying value.
const MAX_POINTS: usize = 100_000;

/// Reads an AAF file into an [`Import`].
pub fn read(bytes: &[u8], opts: &Options) -> Result<Import, Error> {
    let r = Reader::open(bytes)?;
    let sr = opts.sample_rate.max(1);
    let mut w = Walk { r: &r, sr, opts: *opts, imp: Import { sample_rate: sr, ..Import::default() }, media: HashMap::new(), clips: 0 };
    let (company, product, version) = r.writer();
    w.imp.writer = [company, product, version].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
    if opts.pt_auto && is_pro_tools(&w.imp.writer) {
        w.opts.pt_true_fades = true;
    }
    let top = w.top_composition().ok_or_else(|| Error::new("the file holds no composition"))?;
    w.imp.name = top.string(pid::MOB_NAME).unwrap_or_default();
    w.composition(&top);
    w.media_summary();
    Ok(w.imp)
}

/// Where a clip's channel comes from, followed to the essence.
#[derive(Debug, Clone)]
enum Source {
    /// A channel of a media file, `offset` seconds in.
    Media { media: usize, channel: u16, offset: f64, name: String, original: Option<(usize, u16, f64)> },
    /// A slot of another composition (a nested sequence or a subclip), `offset` edit units of
    /// that slot in.
    Nested { mob: MobId, slot: u32, offset: f64 },
    /// Nothing reachable: the name of what it was.
    Missing { name: String },
}

/// One piece of a sequence while it is laid out.
#[derive(Debug)]
struct Entry {
    clips: Vec<Clip>,
}

/// What an operation group does to the segment it wraps.
#[derive(Debug, Default, Clone)]
struct Effect {
    gain: Option<f64>,
    /// Envelope: (normalized time 0..1 of the group, factor).
    envelope: Vec<(f64, f64)>,
    pan: Option<f64>,
    pan_env: Vec<(f64, f64)>,
}

struct Walk<'r, 'a> {
    r: &'r Reader<'a>,
    sr: u32,
    opts: Options,
    imp: Import,
    /// Media key (location or embedded mob) → index in `imp.media`.
    media: HashMap<String, usize>,
    clips: usize,
}

fn is_sound(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("sound") && !n.contains("master")
}

impl<'r, 'a> Walk<'r, 'a> {
    fn skip(&mut self, what: impl Into<String>) {
        let s = what.into();
        if !self.imp.skipped.contains(&s) && self.imp.skipped.len() < 500 {
            self.imp.skipped.push(s);
        }
    }

    /// The composition the file is about: the one marked top-level, else one no other
    /// composition uses, else the first.
    fn top_composition(&self) -> Option<Object> {
        let comps: Vec<&Object> = self.r.mobs().filter(|m| self.r.is(m, "CompositionMob")).collect();
        // Usage_TopLevel ends `...0007` in the usage code's identifier.
        if let Some(m) = comps.iter().find(|m| m.auid(pid::MOB_USAGE).is_some_and(|u| u[..8] == [0x02, 0x01, 0x01, 0x0d, 0x01, 0x01, 0x00, 0x07])) {
            return Some((*m).clone());
        }
        let mut used: Vec<MobId> = Vec::new();
        for m in &comps {
            let mut stack: Vec<(Object, usize)> = vec![((*m).clone(), 0)];
            while let Some((o, d)) = stack.pop() {
                if d > MAX_DEPTH || used.len() > 100_000 {
                    continue;
                }
                if let Some(id) = mob_id(&o, pid::SOURCEREF_ID) {
                    used.push(id);
                }
                for p in &o.props {
                    match p.form {
                        crate::object::SF_STRONG_REF => {
                            if let Some(c) = self.r.store.strong(&o, p.pid) {
                                stack.push((c, d + 1));
                            }
                        }
                        crate::object::SF_STRONG_REF_VECTOR | crate::object::SF_STRONG_REF_SET => {
                            stack.extend(self.r.store.strong_list(&o, p.pid).into_iter().map(|c| (c, d + 1)));
                        }
                        _ => {}
                    }
                }
            }
        }
        comps.iter().find(|m| mob_id(m, pid::MOB_ID).is_some_and(|id| !used.contains(&id))).or(comps.first()).map(|m| (*m).clone())
    }

    fn slots(&self, mob: &Object) -> Vec<Object> {
        self.r.store.strong_list(mob, pid::MOB_SLOTS)
    }

    fn data_def(&self, o: &Object) -> String {
        o.weak_auid(pid::COMPONENT_DATA_DEF).and_then(|a| self.r.def_name(&a).map(str::to_string)).unwrap_or_default()
    }

    /// An object's tagged attributes: the specification's attribute list and every extension
    /// list of tagged values (Avid's `ComponentAttributeList` and `TimelineMobAttributeList`,
    /// Premiere's `TimelineMobSlotAttributes`, ...).
    fn attributes(&self, o: &Object) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        for p in &o.props {
            if p.form == crate::object::SF_STRONG_REF_VECTOR && (p.pid >= 0x8000 || p.pid == pid::COMPONENT_ATTRIBUTES) {
                out.extend(self.r.tagged(o, p.pid));
            }
        }
        out
    }

    fn composition(&mut self, comp: &Object) {
        let slots = self.slots(comp);
        // The start timecode.
        for s in &slots {
            let Some(seg) = self.r.store.strong(s, pid::SLOT_SEGMENT) else { continue };
            let tc = if self.r.is(&seg, "Timecode") {
                Some(seg)
            } else if self.r.is(&seg, "Sequence") {
                self.r.store.strong_list(&seg, pid::SEQUENCE_COMPONENTS).into_iter().find(|c| self.r.is(c, "Timecode"))
            } else {
                None
            };
            if let Some(tc) = tc {
                let rate = s.rational(pid::TIMELINE_SLOT_EDIT_RATE).unwrap_or((25, 1));
                let frames = tc.i64(pid::TIMECODE_START).unwrap_or(0);
                let fps = tc.u16(pid::TIMECODE_FPS).unwrap_or(0);
                let drop = tc.u8(pid::TIMECODE_DROP).unwrap_or(0) != 0;
                self.imp.timecode = Some(Timecode { frames, fps, drop, rate });
                self.imp.start = to_samples(frames, rate, self.sr).max(0);
                break;
            }
        }
        let mut video = 0usize;
        for s in &slots {
            let Some(seg) = self.r.store.strong(s, pid::SLOT_SEGMENT) else { continue };
            if self.r.is(s, "EventMobSlot") {
                self.markers(s, &seg);
                continue;
            }
            if !self.r.is(s, "TimelineMobSlot") {
                continue;
            }
            let dd = self.data_def(&seg);
            if dd.to_ascii_lowercase().contains("descriptive") {
                self.markers(s, &seg);
                continue;
            }
            if self.r.is(&seg, "Timecode") || dd.to_ascii_lowercase().contains("timecode") {
                continue;
            }
            if dd.to_ascii_lowercase().contains("picture") {
                video += 1;
                continue;
            }
            if !is_sound(&dd) {
                if dd.to_ascii_lowercase().contains("soundmaster") || dd.is_empty() {
                    continue;
                }
                self.skip(format!("A {dd} track: only audio is imported"));
                continue;
            }
            self.track(s, &seg);
        }
        if video > 0 {
            self.skip(format!("{video} video track{}: only audio is imported", if video == 1 { "" } else { "s" }));
        }
    }

    fn track(&mut self, slot: &Object, seg: &Object) {
        let rate = slot.rational(pid::TIMELINE_SLOT_EDIT_RATE).filter(|r| r.0 > 0 && r.1 > 0).unwrap_or((25, 1));
        let origin = slot.i64(pid::TIMELINE_SLOT_ORIGIN).unwrap_or(0);
        let n = self.imp.tracks.len() + 1;
        let name = slot.string(pid::SLOT_NAME).filter(|s| !s.trim().is_empty()).unwrap_or_else(|| format!("Audio {n}"));
        let mut t = Track { name, ..Track::default() };
        for (k, v) in self.attributes(slot) {
            match k.as_str() {
                "AudioMixerCompMute" => t.mute = v.as_f64().is_some_and(|x| x != 0.0),
                "AudioMixerCompSolo" => t.solo = v.as_f64().is_some_and(|x| x != 0.0),
                _ => {}
            }
        }
        let at = -to_samples(origin, rate, self.sr);
        let entries = self.segment(seg, rate, at, &mut t, 0);
        let mut clips: Vec<Clip> = entries.into_iter().flat_map(|e| e.clips).collect();
        clips.retain(|c| c.len > 0);
        let eu = to_samples(1, rate, self.sr).max(1);
        if self.opts.pt_true_fades {
            clips = self.pt_true_fades(clips, eu);
        }
        if self.opts.pt_remove_sae || self.opts.pt_true_fades {
            clips = self.pt_remove_sae(clips, eu);
        }
        t.channels = clips.iter().map(|c| c.channels.len() as u16).max().unwrap_or(1).max(1);
        t.clips = clips;
        self.imp.tracks.push(t);
    }

    /// Lays out `seg` starting at `at` (samples on the track) in edit rate `rate`.
    fn segment(&mut self, seg: &Object, rate: (i32, i32), at: i64, track: &mut Track, depth: usize) -> Vec<Entry> {
        let len_eu = seg.i64(pid::COMPONENT_LENGTH).unwrap_or(0).max(0);
        if depth > MAX_DEPTH {
            self.skip("Segments nested too deeply");
            return vec![Entry { clips: Vec::new() }];
        }
        if self.r.is(seg, "Sequence") {
            return self.sequence(seg, rate, at, track, depth);
        }
        if self.r.is(seg, "SourceClip") {
            let clips = self.source_clip(seg, rate, at, len_eu, depth);
            return vec![Entry { clips }];
        }
        if self.r.is(seg, "Filler") {
            return vec![Entry { clips: Vec::new() }];
        }
        if self.r.is(seg, "OperationGroup") {
            return self.operation(seg, rate, at, len_eu, track, depth);
        }
        if self.r.is(seg, "Selector") {
            // The selected segment; Media Composer mutes a clip by selecting a filler in its
            // place and flagging the selector `_DISABLE_CLIP_FLAG`: the clip comes in muted.
            let selected = self.r.store.strong(seg, pid::SELECTOR_SELECTED);
            let disabled = self.attributes(seg).iter().any(|(k, v)| k == "_DISABLE_CLIP_FLAG" && v.as_f64().is_some_and(|x| x != 0.0));
            if disabled
                && selected.as_ref().is_none_or(|s| self.r.is(s, "Filler"))
                && let Some(alt) = self.r.store.strong_list(seg, pid::SELECTOR_ALTERNATES).into_iter().find(|a| !self.r.is(a, "Filler"))
            {
                let mut e = self.segment(&alt, rate, at, track, depth + 1);
                for c in e.iter_mut().flat_map(|e| e.clips.iter_mut()) {
                    c.muted = true;
                }
                return e;
            }
            if let Some(c) = selected {
                return self.segment(&c, rate, at, track, depth + 1);
            }
        }
        if self.r.is(seg, "EssenceGroup") {
            let first = seg
                .props
                .iter()
                .find(|p| p.form == crate::object::SF_STRONG_REF_VECTOR)
                .and_then(|p| self.r.store.strong_list(seg, p.pid).into_iter().next());
            if let Some(c) = first {
                return self.segment(&c, rate, at, track, depth + 1);
            }
        }
        if self.r.is(seg, "NestedScope") {
            // The scope's value is its last slot.
            let inner = seg
                .props
                .iter()
                .find(|p| p.form == crate::object::SF_STRONG_REF_VECTOR)
                .and_then(|p| self.r.store.strong_list(seg, p.pid).into_iter().next_back());
            if let Some(c) = inner {
                return self.segment(&c, rate, at, track, depth + 1);
            }
        }
        let class = self.r.class_name(seg);
        if !matches!(class.as_str(), "Timecode" | "ScopeReference" | "EdgeCode" | "Pulldown") {
            self.skip(format!("A {class} on track \"{}\" (not read)", track.name));
        }
        vec![Entry { clips: Vec::new() }]
    }

    fn sequence(&mut self, seq: &Object, rate: (i32, i32), at: i64, track: &mut Track, depth: usize) -> Vec<Entry> {
        let comps = self.r.store.strong_list(seq, pid::SEQUENCE_COMPONENTS);
        let mut entries: Vec<Entry> = Vec::new();
        let mut pos_eu: i64 = 0;
        // (index of the entry before, length, curve) of each transition.
        let mut transitions: Vec<(usize, i64, Curve)> = Vec::new();
        let base = at;
        for c in comps {
            let len = c.i64(pid::COMPONENT_LENGTH).unwrap_or(0).clamp(0, 1 << 48);
            if self.r.is(&c, "Transition") {
                pos_eu = pos_eu.saturating_sub(len);
                let curve = self.transition_curve(&c);
                transitions.push((entries.len(), len, curve));
                continue;
            }
            let start = base.saturating_add(to_samples(pos_eu, rate, self.sr));
            let mut e = self.segment(&c, rate, start, track, depth + 1);
            if e.is_empty() {
                e.push(Entry { clips: Vec::new() });
            }
            entries.append(&mut e);
            // The segment's own length places what follows, whatever it holds.
            pos_eu = pos_eu.saturating_add(len);
        }
        for (i, len, curve) in transitions {
            let n = to_samples(len, rate, self.sr);
            if n <= 0 {
                continue;
            }
            let prev = i.checked_sub(1).and_then(|k| entries.get(k)).is_some_and(|e| !e.clips.is_empty());
            let next = entries.get(i).is_some_and(|e| !e.clips.is_empty());
            let cross = prev && next;
            if prev && let Some(c) = entries.get_mut(i - 1).and_then(|e| e.clips.last_mut()) {
                c.fade_out = Some(Fade { len: n.min(c.len), curve, cross });
            }
            if next && let Some(c) = entries.get_mut(i).and_then(|e| e.clips.first_mut()) {
                c.fade_in = Some(Fade { len: n.min(c.len), curve, cross });
            }
        }
        entries
    }

    /// The fade shape of a transition: Avid's `Curve Type` (0 linear, 1 equal power), else the
    /// interpolation of its level parameter (linear, power, logarithmic).
    fn transition_curve(&self, t: &Object) -> Curve {
        let Some(op) = self.r.store.strong(t, pid::TRANSITION_OPGROUP) else { return Curve::Linear };
        let mut curve = None;
        for p in self.r.store.strong_list(&op, pid::OPGROUP_PARAMETERS) {
            let name = p.auid(pid::PARAMETER_DEFINITION).and_then(|a| self.r.def_name(&a).map(str::to_string)).unwrap_or_default();
            if name == "Curve Type"
                && let Some(v) = p.data(pid::CONSTANT_VALUE).map(indirect).and_then(|v| v.as_f64())
            {
                return if v >= 1.0 { Curve::Power } else { Curve::Linear };
            }
            if let Some(i) = p.weak_auid(pid::VARYING_INTERPOLATION) {
                let n = self.r.def_name(&i).unwrap_or("").to_ascii_lowercase();
                curve = Some(if n.contains("log") {
                    Curve::Log
                } else if n.contains("power") {
                    Curve::Power
                } else if n.contains("exp") {
                    Curve::Exp
                } else {
                    Curve::Linear
                });
            }
        }
        curve.unwrap_or(Curve::Linear)
    }

    fn operation(&mut self, op: &Object, rate: (i32, i32), at: i64, len_eu: i64, track: &mut Track, depth: usize) -> Vec<Entry> {
        let end = at.saturating_add(to_samples(len_eu, rate, self.sr));
        let name = op.weak_auid(pid::OPGROUP_OPERATION).and_then(|a| self.r.def_name(&a).map(str::to_string)).unwrap_or_default();
        let lname = name.to_ascii_lowercase();
        let inputs = self.r.store.strong_list(op, pid::OPGROUP_INPUTS);
        if lname.contains("channel combiner") {
            // One input per channel, each a source clip.
            let mut chans: Vec<Option<ChannelRef>> = Vec::new();
            let mut clip_name = String::new();
            let mut offset = None;
            for i in &inputs {
                let clips = if self.r.is(i, "SourceClip") { self.source_clip(i, rate, at, len_eu, depth + 1) } else { Vec::new() };
                match clips.into_iter().next() {
                    Some(c) => {
                        if clip_name.is_empty() {
                            clip_name = c.name.clone();
                        }
                        offset.get_or_insert(c.offset);
                        chans.extend(c.channels);
                    }
                    None => chans.push(None),
                }
            }
            if inputs.is_empty() {
                return vec![Entry { clips: Vec::new() }];
            }
            let clip = Clip { name: clip_name, start: at, len: end - at, offset: offset.unwrap_or(0), channels: chans, ..Clip::default() };
            return vec![Entry { clips: vec![clip] }];
        }
        let effect = self.effect(op);
        let Some(input) = inputs.first() else {
            if !name.is_empty() {
                self.skip(format!("Effect \"{name}\" with no input"));
            }
            return vec![Entry { clips: Vec::new() }];
        };
        let wraps_sequence = self.r.is(input, "Sequence");
        let mut entries = self.segment(input, rate, at, track, depth + 1);
        // What the group's input holds past the group's own length is not heard.
        for e in &mut entries {
            e.clips.retain_mut(|c| {
                let cend = (c.start + c.len).min(end);
                if cend < c.start + c.len {
                    c.len = cend - c.start;
                    c.fade_out = None;
                }
                c.len > 0
            });
        }
        if lname.contains("motion") || lname.contains("warp") || lname.contains("speed") {
            self.skip(format!("Speed change \"{name}\" (the clip plays at its own speed)"));
        }
        let gain_like = effect.gain.is_some() || !effect.envelope.is_empty();
        let pan_like = effect.pan.is_some() || !effect.pan_env.is_empty();
        let group_len = (end - at).max(1);
        if wraps_sequence {
            // A track-level effect: track volume and pan.
            if gain_like {
                if let Some(g) = effect.gain {
                    track.gain_db = Some(to_db(g));
                }
                track.gain_auto = effect.envelope.iter().map(|(t, v)| (at + samples_f(t * group_len as f64), *v)).collect();
            }
            if pan_like {
                if let Some(p) = effect.pan {
                    track.pan = Some(pan_of(p));
                }
                track.pan_auto = effect.pan_env.iter().map(|(t, v)| (at + samples_f(t * group_len as f64), pan_of(*v))).collect();
            }
        } else {
            for c in entries.iter_mut().flat_map(|e| e.clips.iter_mut()) {
                if gain_like {
                    // Gains wrapped round one another multiply.
                    if let Some(g) = effect.gain {
                        c.gain_db = Some((c.gain_db.unwrap_or(0.0) + to_db(g)).clamp(-144.0, 24.0));
                    }
                    if !effect.envelope.is_empty() && c.gain_env.is_empty() {
                        c.gain_env = effect.envelope.iter().map(|(t, v)| (at + samples_f(t * group_len as f64) - c.start, *v)).collect();
                    }
                }
                if pan_like {
                    self.imp.skipped.retain(|s| !s.starts_with("Clip pan"));
                    self.imp.skipped.push("Clip pan (FreeDAWm pans tracks, not clips)".into());
                }
            }
        }
        if !gain_like && !pan_like && !name.is_empty() {
            self.skip(format!("Effect \"{name}\" (the clip plays without it)"));
        }
        entries
    }

    /// The gain and pan an operation group sets.
    fn effect(&self, op: &Object) -> Effect {
        let mut e = Effect::default();
        for p in self.r.store.strong_list(op, pid::OPGROUP_PARAMETERS) {
            let name = p.auid(pid::PARAMETER_DEFINITION).and_then(|a| self.r.def_name(&a).map(str::to_string)).unwrap_or_default();
            let ln = name.to_ascii_lowercase();
            let is_gain = ln.starts_with("amplitude") || ln == "avidcontrolclipratio" || ln == "level";
            let is_pan = ln == "pan" || ln == "pan value";
            if !is_gain && !is_pan {
                continue;
            }
            let constant = p.data(pid::CONSTANT_VALUE).map(indirect).and_then(|v| v.as_f64()).filter(|v| v.is_finite());
            let points: Vec<(f64, f64)> = self
                .r
                .store
                .strong_list(&p, pid::VARYING_POINTS)
                .into_iter()
                .take(MAX_POINTS)
                .filter_map(|cp| {
                    let (n, d) = cp.rational(pid::CONTROL_POINT_TIME)?;
                    let v = indirect(cp.data(pid::CONTROL_POINT_VALUE)?).as_f64()?;
                    (d != 0 && v.is_finite()).then(|| (f64::from(n) / f64::from(d), v))
                })
                .collect();
            // A varying value whose points are all one value is that constant.
            let (constant, points) = match (constant, points.first()) {
                (None, Some(&(_, v0))) if points.iter().all(|(_, v)| (v - v0).abs() < 1e-9) => (Some(v0), Vec::new()),
                _ => (constant, points),
            };
            if is_gain && ln != "level" {
                if let Some(v) = constant {
                    e.gain = Some(v);
                }
                if !points.is_empty() {
                    e.envelope = points;
                }
            } else if is_pan {
                if let Some(v) = constant {
                    e.pan = Some(v);
                }
                if !points.is_empty() {
                    e.pan_env = points;
                }
            }
        }
        e
    }

    /// The clips a source clip at `at` places (one, or a nested composition's).
    fn source_clip(&mut self, sc: &Object, rate: (i32, i32), at: i64, len_eu: i64, depth: usize) -> Vec<Clip> {
        if self.clips >= MAX_CLIPS {
            self.skip(format!("Clips past the first {MAX_CLIPS}"));
            return Vec::new();
        }
        let len = to_samples(len_eu, rate, self.sr);
        if len <= 0 {
            return Vec::new();
        }
        let start_eu = sc.i64(pid::SOURCECLIP_START).unwrap_or(0);
        let Some(id) = mob_id(sc, pid::SOURCEREF_ID).filter(|id| id.iter().any(|b| *b != 0)) else {
            // A source clip naming no mob is silence.
            return Vec::new();
        };
        let slot = sc.u32(pid::SOURCEREF_SLOT).unwrap_or(0);
        let mut fades = (None, None);
        if let Some(l) = sc.i64(pid::SOURCECLIP_FADE_IN_LEN).filter(|l| *l > 0) {
            fades.0 = Some(Fade { len: to_samples(l, rate, self.sr).min(len), curve: fade_type(sc.u32(pid::SOURCECLIP_FADE_IN_TYPE)), cross: false });
        }
        if let Some(l) = sc.i64(pid::SOURCECLIP_FADE_OUT_LEN).filter(|l| *l > 0) {
            fades.1 =
                Some(Fade { len: to_samples(l, rate, self.sr).min(len), curve: fade_type(sc.u32(pid::SOURCECLIP_FADE_OUT_TYPE)), cross: false });
        }
        let src = self.resolve(id, slot, start_eu as f64 * f64::from(rate.1) / f64::from(rate.0), depth);
        let mut out = match src {
            Source::Media { media, channel, offset, name, original } => {
                let off = samples_f(offset * f64::from(self.sr));
                let original = original.map(|(m, c, o)| (m, c, samples_f(o * f64::from(self.sr))));
                vec![Clip {
                    name,
                    start: at,
                    len,
                    offset: off,
                    channels: vec![Some(ChannelRef { media, channel, offset: off, original })],
                    ..Clip::default()
                }]
            }
            Source::Missing { name } => {
                vec![Clip { name, start: at, len, channels: vec![None], ..Clip::default() }]
            }
            Source::Nested { mob, slot, offset } => self.nested(mob, slot, offset, at, len, depth),
        };
        for c in &mut out {
            if fades.0.is_some() {
                c.fade_in = fades.0;
            }
            if fades.1.is_some() {
                c.fade_out = fades.1;
            }
        }
        self.clips += out.len();
        out
    }

    /// A nested composition's clips between `offset` (seconds into its slot) and `len` samples
    /// later, placed at `at`.
    fn nested(&mut self, mob: MobId, slot_id: u32, offset: f64, at: i64, len: i64, depth: usize) -> Vec<Clip> {
        let Some(m) = self.r.mob(&mob).cloned() else { return Vec::new() };
        let name = m.string(pid::MOB_NAME).unwrap_or_default();
        let Some(slot) = self.slots(&m).into_iter().find(|s| s.u32(pid::SLOT_ID) == Some(slot_id)) else { return Vec::new() };
        let Some(seg) = self.r.store.strong(&slot, pid::SLOT_SEGMENT) else { return Vec::new() };
        let rate = slot.rational(pid::TIMELINE_SLOT_EDIT_RATE).filter(|r| r.0 > 0 && r.1 > 0).unwrap_or((25, 1));
        let shift = at - samples_f(offset * f64::from(self.sr));
        let mut scratch = Track::default();
        let entries = self.segment(&seg, rate, shift, &mut scratch, depth + 1);
        let (lo, hi) = (at, at + len);
        let mut out = Vec::new();
        for mut c in entries.into_iter().flat_map(|e| e.clips) {
            // Windowed to the part the outer clip uses.
            let (s, e) = (c.start.max(lo), (c.start + c.len).min(hi));
            if e <= s {
                continue;
            }
            let cut = s - c.start;
            c.offset += cut;
            for ch in c.channels.iter_mut().flatten() {
                ch.shift(cut);
            }
            c.gain_env = c.gain_env.iter().map(|(t, v)| (t - cut, *v)).collect();
            if cut > 0 {
                c.fade_in = None;
            }
            if e < c.start + c.len {
                c.fade_out = None;
            }
            c.start = s;
            c.len = e - s;
            // A subclip's own clip gain is not carried: the clip's gain is the one the outer
            // composition gives it (as Media Composer plays it).
            c.gain_db = None;
            c.gain_env.clear();
            // A subclip's clips carry its name.
            if self.r.is(&m, "CompositionMob") && !name.is_empty() {
                c.name = name.clone();
            }
            out.push(c);
        }
        out
    }

    /// Follows mob `id`'s slot `slot`, `start` seconds in, to the essence. A source clip's start
    /// counts in the edit rate of the slot holding the clip.
    fn resolve(&mut self, mut id: MobId, mut slot_id: u32, start: f64, depth: usize) -> Source {
        let mut name = String::new();
        // Position in the current slot, in seconds.
        let mut here = start;
        for _ in 0..MAX_DEPTH.saturating_sub(depth).max(1) {
            let Some(m) = self.r.mob(&id).cloned() else { return Source::Missing { name } };
            if name.is_empty() {
                name = m.string(pid::MOB_NAME).unwrap_or_default();
            }
            if self.r.is(&m, "CompositionMob") {
                return Source::Nested { mob: id, slot: slot_id, offset: here };
            }
            let slots = self.slots(&m);
            let Some(slot) = slots.iter().find(|s| s.u32(pid::SLOT_ID) == Some(slot_id)).or(if slots.len() == 1 { slots.first() } else { None })
            else {
                return Source::Missing { name };
            };
            let rate = slot.rational(pid::TIMELINE_SLOT_EDIT_RATE).filter(|r| r.0 > 0 && r.1 > 0).unwrap_or((25, 1));
            if self.r.is(&m, "SourceMob")
                && let Some(desc) = self.r.store.strong(&m, pid::SOURCE_MOB_DESCRIPTOR)
                && self.is_file_descriptor(&desc)
            {
                let phys = slot.u32(pid::SLOT_PHYSICAL_TRACK).unwrap_or(1).max(1);
                let Some(media) = self.media_of(&m, &desc, &name) else { return Source::Missing { name } };
                let chans = self.imp.media.get(media).map_or(1, |x| x.channels.max(1));
                let channel = if chans > 1 { u16::try_from(phys - 1).unwrap_or(0).min(chans - 1) } else { 0 };
                let original = self.original(slot, rate, here);
                return Source::Media { media, channel, offset: here, name, original };
            }
            let Some(seg) = self.r.store.strong(slot, pid::SLOT_SEGMENT) else { return Source::Missing { name } };
            // The segment at `here` within this slot.
            let mut seg = seg;
            let mut in_seg = here;
            for _ in 0..4 {
                if self.r.is(&seg, "Sequence") {
                    let mut pos = 0.0;
                    let mut found = None;
                    for c in self.r.store.strong_list(&seg, pid::SEQUENCE_COMPONENTS) {
                        let l = c.i64(pid::COMPONENT_LENGTH).unwrap_or(0) as f64 * f64::from(rate.1) / f64::from(rate.0);
                        if self.r.is(&c, "Transition") {
                            pos -= l;
                            continue;
                        }
                        if in_seg < pos + l || found.is_none() {
                            found = Some((c, pos));
                            if in_seg < pos + l {
                                break;
                            }
                        }
                        pos += l;
                    }
                    let Some((c, p)) = found else { return Source::Missing { name } };
                    in_seg -= p;
                    seg = c;
                } else if self.r.is(&seg, "EssenceGroup") || self.r.is(&seg, "Selector") || self.r.is(&seg, "OperationGroup") {
                    let next = seg
                        .props
                        .iter()
                        .filter(|p| p.form == crate::object::SF_STRONG_REF_VECTOR || p.form == crate::object::SF_STRONG_REF)
                        .find_map(|p| {
                            if p.form == crate::object::SF_STRONG_REF {
                                self.r.store.strong(&seg, p.pid).filter(|o| self.r.is(o, "Segment"))
                            } else {
                                self.r.store.strong_list(&seg, p.pid).into_iter().find(|o| self.r.is(o, "Segment"))
                            }
                        });
                    let Some(n) = next else { return Source::Missing { name } };
                    seg = n;
                } else {
                    break;
                }
            }
            if !self.r.is(&seg, "SourceClip") {
                return Source::Missing { name };
            }
            let Some(next) = mob_id(&seg, pid::SOURCEREF_ID).filter(|id| id.iter().any(|b| *b != 0)) else { return Source::Missing { name } };
            // The next slot's position: this clip's start there (in this slot's rate), plus how far
            // into the clip we are.
            here = in_seg + seg.i64(pid::SOURCECLIP_START).unwrap_or(0) as f64 * f64::from(rate.1) / f64::from(rate.0);
            slot_id = seg.u32(pid::SOURCEREF_SLOT).unwrap_or(0);
            id = next;
        }
        self.skip("Source chains longer than 24 mobs");
        Source::Missing { name }
    }

    /// The file a file source mob's media was imported from (Media Composer keeps an import
    /// source mob with the original file's location behind its own MXF media): (media, channel,
    /// seconds in).
    fn original(&mut self, slot: &Object, rate: (i32, i32), here: f64) -> Option<(usize, u16, f64)> {
        let seg = self.r.store.strong(slot, pid::SLOT_SEGMENT).filter(|s| self.r.is(s, "SourceClip"))?;
        let id = mob_id(&seg, pid::SOURCEREF_ID).filter(|id| id.iter().any(|b| *b != 0))?;
        let m = self.r.mob(&id).cloned().filter(|m| self.r.is(m, "SourceMob"))?;
        let desc = self.r.store.strong(&m, pid::SOURCE_MOB_DESCRIPTOR).filter(|d| self.r.is(d, "ImportDescriptor") || self.is_file_descriptor(d))?;
        if self.r.store.strong_list(&desc, pid::DESCRIPTOR_LOCATORS).is_empty() {
            return None;
        }
        let want = seg.u32(pid::SOURCEREF_SLOT).unwrap_or(0);
        let slots = self.slots(&m);
        let s = slots.iter().find(|s| s.u32(pid::SLOT_ID) == Some(want))?;
        let phys = s.u32(pid::SLOT_PHYSICAL_TRACK).unwrap_or(1).max(1);
        let seconds = here + seg.i64(pid::SOURCECLIP_START).unwrap_or(0) as f64 * f64::from(rate.1) / f64::from(rate.0);
        let name = m.string(pid::MOB_NAME).unwrap_or_default();
        let media = self.media_of(&m, &desc, &name)?;
        Some((media, u16::try_from(phys - 1).unwrap_or(0), seconds))
    }

    fn is_file_descriptor(&self, d: &Object) -> bool {
        let chain = self.r.class_chain(&d.class);
        chain
            .iter()
            .any(|c| c == "FileDescriptor" || c == "SoundDescriptor" || c == "WAVEDescriptor" || c == "AIFCDescriptor" || c == "PCMDescriptor")
    }

    /// The media a file source mob's essence is, added once per location.
    fn media_of(&mut self, sm: &Object, desc: &Object, name: &str) -> Option<usize> {
        let id = mob_id(sm, pid::MOB_ID)?;
        let class = self.r.class_chain(&desc.class);
        let has = |c: &str| class.iter().any(|x| x == c);
        let embedded = self.r.essence_data(&id).cloned();
        let locations: Vec<String> = self
            .r
            .store
            .strong_list(desc, pid::DESCRIPTOR_LOCATORS)
            .into_iter()
            .filter_map(|l| l.string(pid::NETWORK_LOCATOR_URL).or_else(|| l.string(pid::TEXT_LOCATOR_NAME)))
            .filter(|s| !s.is_empty())
            .collect();
        let key = match (&embedded, locations.first()) {
            (Some(_), _) => format!("embedded:{}", id.iter().map(|b| format!("{b:02x}")).collect::<String>()),
            (None, Some(l)) => format!("url:{l}"),
            (None, None) => format!("mob:{}", id.iter().map(|b| format!("{b:02x}")).collect::<String>()),
        };
        if let Some(&i) = self.media.get(&key) {
            return Some(i);
        }
        // Format facts: the descriptor's own, else the WAVE / AIFC summary header.
        let mut rate = desc.rational(pid::FILE_SAMPLE_RATE).or_else(|| desc.rational(pid::SOUND_SAMPLING_RATE)).filter(|r| r.0 > 0 && r.1 > 0);
        let mut channels = desc.u32(pid::SOUND_CHANNELS).and_then(|c| u16::try_from(c).ok()).filter(|c| *c > 0);
        let mut bits = desc.u32(pid::SOUND_QUANTIZATION).and_then(|c| u16::try_from(c).ok()).filter(|c| *c > 0);
        let summary = if has("WAVEDescriptor") { desc.data(pid::WAVE_SUMMARY).and_then(wave_format) } else { None }
            .or_else(|| if has("AIFCDescriptor") { desc.data(pid::AIFC_SUMMARY).and_then(aifc_format) } else { None });
        if let Some((c, r, b)) = summary {
            channels = channels.or(Some(c));
            bits = bits.or(Some(b));
            if rate.is_none() && r > 0 {
                rate = Some((r as i32, 1));
            }
        }
        let sample_rate = rate.map_or(0, |(n, d)| (f64::from(n) / f64::from(d)).round() as u32);
        let frames = desc.i64(pid::FILE_LENGTH).and_then(|l| u64::try_from(l).ok()).unwrap_or(0);
        let channels = channels.unwrap_or(1);
        let bits = bits.unwrap_or(16);
        let data = match embedded {
            Some(e) => match self.r.store.stream(&e, pid::ESSENCE_DATA_DATA, self.opts.max_embedded) {
                Ok(bytes) => {
                    if has("WAVEDescriptor") || bytes.starts_with(b"RIFF") || bytes.starts_with(b"RF64") {
                        MediaData::Embedded { bytes, ext: "wav" }
                    } else if has("AIFCDescriptor") || bytes.starts_with(b"FORM") {
                        MediaData::Embedded { bytes, ext: "aiff" }
                    } else if matches!(bits, 8 | 16 | 24 | 32) && sample_rate > 0 {
                        MediaData::Embedded { bytes: pcm_wav(&bytes, channels, sample_rate, bits), ext: "wav" }
                    } else {
                        self.skip(format!("Embedded audio \"{name}\" in a format not read ({bits}-bit)"));
                        MediaData::Missing
                    }
                }
                Err(e) => {
                    self.skip(format!("Embedded audio \"{name}\": {e}"));
                    MediaData::Missing
                }
            },
            None if !locations.is_empty() => MediaData::External { locations },
            None => MediaData::Missing,
        };
        let i = self.imp.media.len();
        self.imp.media.push(Media { name: name.to_string(), channels, sample_rate, bits, frames, data });
        self.media.insert(key, i);
        Some(i)
    }

    /// The length of a clip's media, in samples at the import rate (`None` when not known).
    fn media_len(&self, c: &Clip) -> Option<i64> {
        let m = self.imp.media.get(c.channels.first().copied().flatten()?.media)?;
        (m.frames > 0 && m.sample_rate > 0).then(|| samples_f(m.frames as f64 * f64::from(self.sr) / f64::from(m.sample_rate)))
    }

    /// A Pro Tools "sample accurate edit": a clip of at most one edit unit playing a file that
    /// is not much longer.
    fn is_sae(&self, c: &Clip, eu: i64) -> bool {
        c.len <= eu + 1 && self.media_len(c).is_some_and(|l| l <= 2 * eu + 2)
    }

    /// A Pro Tools rendered fade: a clip named as Pro Tools names them, or one playing a file
    /// barely longer than itself from its start with a "sample accurate edit" against it.
    fn is_pt_fade(&self, clips: &[Clip], i: usize, eu: i64) -> bool {
        let Some(c) = clips.get(i) else { return false };
        if self.is_sae(c, eu) {
            return false;
        }
        let touch = |a: &Clip, b: &Clip| (a.start + a.len - b.start).abs() <= 1;
        let flanked = i.checked_sub(1).and_then(|k| clips.get(k)).is_some_and(|p| self.is_sae(p, eu) && touch(p, c))
            || clips.get(i + 1).is_some_and(|n| self.is_sae(n, eu) && touch(c, n));
        let named = PT_FADE_NAMES.contains(&c.name.trim_end());
        let rendered = c.offset <= eu + 1 && self.media_len(c).is_some_and(|l| l <= c.len + 2 * eu + 2);
        flanked && (named || rendered)
    }

    /// Can `c` start `by` samples earlier and still be inside its media?
    fn can_extend_back(c: &Clip, by: i64) -> bool {
        c.offset >= by && c.channels.iter().flatten().all(|ch| ch.offset >= by)
    }

    fn can_extend_end(&self, c: &Clip, by: i64) -> bool {
        self.media_len(c).is_none_or(|l| c.offset + c.len + by <= l + 1)
    }

    fn pt_true_fades(&mut self, mut clips: Vec<Clip>, eu: i64) -> Vec<Clip> {
        let mut gone = vec![false; clips.len()];
        let touch = |a: &Clip, b: &Clip| (a.start + a.len - b.start).abs() <= 1;
        for f in 0..clips.len() {
            if !self.is_pt_fade(&clips, f, eu) {
                continue;
            }
            let l = f.checked_sub(1).filter(|&k| !gone[k] && self.is_sae(&clips[k], eu) && touch(&clips[k], &clips[f]));
            let t = Some(f + 1).filter(|&k| k < clips.len() && self.is_sae(&clips[k], eu) && touch(&clips[f], &clips[k]));
            let first = l.unwrap_or(f);
            let last = t.unwrap_or(f);
            let (g0, g1) = (clips[first].start, clips[last].start + clips[last].len);
            let (fs, fe) = (clips[f].start, clips[f].start + clips[f].len);
            let p = first.checked_sub(1).filter(|&k| !gone[k] && !self.is_pt_fade(&clips, k, eu) && (clips[k].start + clips[k].len - g0).abs() <= 1);
            let n = Some(last + 1).filter(|&k| k < clips.len() && !self.is_pt_fade(&clips, k, eu) && (clips[k].start - g1).abs() <= 1);
            let ok = match (p, n) {
                (Some(p), Some(n)) => {
                    self.can_extend_end(&clips[p], g1 - (clips[p].start + clips[p].len)) && Self::can_extend_back(&clips[n], clips[n].start - fs)
                }
                (Some(p), None) => self.can_extend_end(&clips[p], g1 - (clips[p].start + clips[p].len)),
                (None, Some(n)) => Self::can_extend_back(&clips[n], clips[n].start - g0),
                (None, None) => false,
            };
            if !ok {
                continue;
            }
            let back = |c: &mut Clip, to: i64| {
                let by = c.start - to;
                c.start = to;
                c.len += by;
                c.offset -= by;
                for ch in c.channels.iter_mut().flatten() {
                    ch.shift(-by);
                }
                c.gain_env = c.gain_env.iter().map(|(t, v)| (t + by, *v)).collect();
            };
            match (p, n) {
                (Some(p), Some(n)) => {
                    clips[p].len = g1 - clips[p].start;
                    back(&mut clips[n], fs);
                    let x = g1 - fs;
                    clips[p].fade_out = Some(Fade { len: x, curve: Curve::Linear, cross: true });
                    clips[n].fade_in = Some(Fade { len: x, curve: Curve::Linear, cross: true });
                }
                (Some(p), None) => {
                    clips[p].len = g1 - clips[p].start;
                    clips[p].fade_out = Some(Fade { len: fe - g0, curve: Curve::Linear, cross: false });
                }
                (None, Some(n)) => {
                    back(&mut clips[n], g0);
                    clips[n].fade_in = Some(Fade { len: g1 - g0, curve: Curve::Linear, cross: false });
                }
                (None, None) => {}
            }
            for g in gone.iter_mut().take(last + 1).skip(first) {
                *g = true;
            }
        }
        clips.into_iter().zip(gone).filter(|(_, g)| !g).map(|(c, _)| c).collect()
    }

    fn pt_remove_sae(&mut self, mut clips: Vec<Clip>, eu: i64) -> Vec<Clip> {
        let mut gone = vec![false; clips.len()];
        for i in 0..clips.len() {
            if !self.is_sae(&clips[i], eu) {
                continue;
            }
            let (s, len) = (clips[i].start, clips[i].len);
            let prev = (0..i).rev().find(|&k| !gone[k]).filter(|&k| (clips[k].start + clips[k].len - s).abs() <= 1);
            if let Some(k) = prev {
                clips[k].len = s + len - clips[k].start;
                gone[i] = true;
                continue;
            }
            if let Some(k) = Some(i + 1)
                .filter(|&k| k < clips.len() && (clips[k].start - (s + len)).abs() <= 1 && Self::can_extend_back(&clips[k], clips[k].start - s))
            {
                let by = clips[k].start - s;
                let c = &mut clips[k];
                c.start = s;
                c.len += by;
                c.offset -= by;
                for ch in c.channels.iter_mut().flatten() {
                    ch.shift(-by);
                }
                gone[i] = true;
            }
        }
        clips.into_iter().zip(gone).filter(|(_, g)| !g).map(|(c, _)| c).collect()
    }

    fn markers(&mut self, slot: &Object, seg: &Object) {
        let rate = slot
            .rational(pid::EVENT_SLOT_EDIT_RATE)
            .or_else(|| slot.rational(pid::TIMELINE_SLOT_EDIT_RATE))
            .filter(|r| r.0 > 0 && r.1 > 0)
            .unwrap_or((25, 1));
        let events = if self.r.is(seg, "Sequence") { self.r.store.strong_list(seg, pid::SEQUENCE_COMPONENTS) } else { vec![seg.clone()] };
        let user = self.r.ext_pid("CommentMarkerUser");
        let color = self.r.ext_pid("CommentMarkerColor");
        for e in events.into_iter().take(100_000) {
            if !self.r.is(&e, "CommentMarker") && !self.r.is(&e, "DescriptiveMarker") {
                continue;
            }
            let pos = e.i64(pid::EVENT_POSITION).unwrap_or(0);
            let len = e.i64(pid::COMPONENT_LENGTH).unwrap_or(0).max(0);
            let comment = e.string(pid::EVENT_COMMENT).unwrap_or_default();
            let tagged = self.attributes(&e);
            let label = user
                .and_then(|p| e.string(p))
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    tagged.iter().find(|(k, _)| k == "_ATN_CRM_USER").and_then(|(_, v)| if let Value::Text(t) = v { Some(t.clone()) } else { None })
                })
                .unwrap_or_else(|| if comment.is_empty() { format!("Marker {}", self.imp.markers.len() + 1) } else { comment.clone() });
            let rgb = color.and_then(|p| e.data(p)).and_then(|b| (b.len() >= 6).then(|| [b[1], b[3], b[5]]));
            self.imp.markers.push(Marker {
                name: label,
                comment,
                start: to_samples(pos, rate, self.sr),
                len: if len > 1 { to_samples(len, rate, self.sr) } else { 0 },
                color: rgb,
            });
        }
    }

    fn media_summary(&mut self) {
        let mut rates: HashMap<u32, usize> = HashMap::new();
        let mut bits: HashMap<u16, usize> = HashMap::new();
        for m in &self.imp.media {
            if m.sample_rate > 0 {
                *rates.entry(m.sample_rate).or_default() += 1;
            }
            *bits.entry(m.bits).or_default() += 1;
        }
        self.imp.media_rate = rates.into_iter().max_by_key(|(r, n)| (*n, *r)).map(|(r, _)| r);
        self.imp.media_bits = bits.into_iter().max_by_key(|(b, n)| (*n, *b)).map(|(b, _)| b);
    }
}

/// The names Pro Tools gives its rendered fade clips (English, French, Spanish, Korean,
/// simplified and traditional Chinese; German and Japanese use the English one).
const PT_FADE_NAMES: &[&str] = &["Fade", "Fondu", "Fundido", "페이드", "淡变", "淡變"];

/// A sample count from a float, clamped to ±[`MAX_SAMPLES`] (0 for NaN).
fn samples_f(x: f64) -> i64 {
    if x.is_nan() { 0 } else { x.round().clamp(-(MAX_SAMPLES as f64), MAX_SAMPLES as f64) as i64 }
}

/// A gain factor in dB (−144 for silence).
pub fn to_db(g: f64) -> f32 {
    if g <= 0.0 || !g.is_finite() { -144.0 } else { (20.0 * g.log10()).clamp(-144.0, 24.0) as f32 }
}

/// AAF pan (0 left, 0.5 centre, 1 right) as −1..1.
fn pan_of(p: f64) -> f32 {
    if p.is_finite() { (p * 2.0 - 1.0).clamp(-1.0, 1.0) as f32 } else { 0.0 }
}

/// A source clip's fade type (1 linear amplitude, 2 linear power).
fn fade_type(t: Option<u32>) -> Curve {
    if t == Some(2) { Curve::Power } else { Curve::Linear }
}

/// (channels, rate, bits) from a WAVE summary (a RIFF header with its `fmt ` chunk).
fn wave_format(b: &[u8]) -> Option<(u16, u32, u16)> {
    let mut at = 12usize;
    while at + 8 <= b.len() {
        let id = b.get(at..at + 4)?;
        let len = u32::from_le_bytes(b.get(at + 4..at + 8)?.try_into().ok()?) as usize;
        if id == b"fmt " {
            let f = b.get(at + 8..at + 8 + 16.min(len))?;
            let ch = u16::from_le_bytes(f.get(2..4)?.try_into().ok()?);
            let rate = u32::from_le_bytes(f.get(4..8)?.try_into().ok()?);
            let bits = u16::from_le_bytes(f.get(14..16)?.try_into().ok()?);
            return Some((ch, rate, bits));
        }
        at = at.checked_add(8)?.checked_add(len + (len & 1))?;
    }
    None
}

/// (channels, rate, bits) from an AIFC summary (a FORM header with its `COMM` chunk).
fn aifc_format(b: &[u8]) -> Option<(u16, u32, u16)> {
    let mut at = 12usize;
    while at + 8 <= b.len() {
        let id = b.get(at..at + 4)?;
        let len = u32::from_be_bytes(b.get(at + 4..at + 8)?.try_into().ok()?) as usize;
        if id == b"COMM" {
            let c = b.get(at + 8..)?;
            let ch = u16::from_be_bytes(c.get(0..2)?.try_into().ok()?);
            let bits = u16::from_be_bytes(c.get(6..8)?.try_into().ok()?);
            let rate = ieee80(c.get(8..18)?)?;
            return Some((ch, rate, bits));
        }
        at = at.checked_add(8)?.checked_add(len + (len & 1))?;
    }
    None
}

/// An 80-bit IEEE extended float (AIFF sample rates) as a whole number.
fn ieee80(b: &[u8]) -> Option<u32> {
    let exp = i32::from(u16::from_be_bytes([*b.first()?, *b.get(1)?]) & 0x7FFF);
    let mant = u64::from_be_bytes(b.get(2..10)?.try_into().ok()?);
    if exp == 0 || mant == 0 {
        return Some(0);
    }
    let shift = 16383 + 63 - exp;
    if !(0..64).contains(&shift) {
        return None;
    }
    u32::try_from(mant >> shift).ok()
}

/// Raw little-endian PCM given a WAV header.
pub fn pcm_wav(pcm: &[u8], channels: u16, rate: u32, bits: u16) -> Vec<u8> {
    let block = u32::from(channels.max(1)) * u32::from(bits.div_ceil(8));
    let data_len = u32::try_from(pcm.len()).unwrap_or(u32::MAX) / block.max(1) * block.max(1);
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36u32.saturating_add(data_len)).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.max(1).to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&rate.saturating_mul(block).to_le_bytes());
    out.extend_from_slice(&(block as u16).to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(pcm.get(..data_len as usize).unwrap_or(pcm));
    out
}

#[cfg(test)]
mod tests;
