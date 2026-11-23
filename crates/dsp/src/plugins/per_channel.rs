//! Runs a stereo-only plugin on every channel of a wider track.
//!
//! The FireStation engines, Vault and Mend process two channels at most (and the stock compressor
//! and gate linked every channel's detection). On a track with more
//! (LCR, quad, 5.1, 7.1.4, Ambisonics) [`PerChannel`] runs one instance on the first two channels
//! (left and right, as before) and one mono instance on every further channel, so the centre, the
//! LFE and the surrounds are processed too instead of passing dry. On a mono or stereo track it is
//! the plugin itself: every call goes straight to the one instance.
//!
//! The extra instances follow the first: the same parameters, transport, actions, assets and
//! prepared state. Meters, readouts, analysis, links, editors and stored state are the first
//! instance's. Detection is not linked across instances by default: a compressor on 5.1 reduces
//! the front pair, the centre and each surround on its own. A plugin with a key input and a
//! `surLink` parameter (Comp's Link All Channels) can link them: while it is on and no key is
//! chosen, every instance is keyed from one signal, each sample the loudest channel's, so they all
//! measure the same thing and reduce together. The LFE, as the host names it from the track's
//! channel format ([`Plugin::set_lfe_channels`]), is left out: it neither feeds the key nor is
//! turned down by it (its instance detects on its own signal).

use crate::{LinkSlab, MidiOut, Plugin, PluginEditor, PluginInfo, PluginStateReader, SampleSource, Transport, analysis};
use std::sync::Arc;

/// The ids that [`super::instantiate`] wraps: every plugin whose `prepare` caps the channel count
/// at two.
pub(crate) fn wraps(id: &str) -> bool {
    id == super::vault::VAULT_INFO.id || id == super::mend::MEND_INFO.id || super::fs::engine(id).is_some() || LINKED_ON_ALL.contains(&id)
}

/// Stock dynamics that linked their detection across every channel, so a loud LFE ducked the
/// whole surround mix: wrapped, they link the front pair and treat each other channel alone.
const LINKED_ON_ALL: [&str; 2] = ["compressor", "expander_gate"];

/// A stereo-only plugin run on every channel (see the module docs).
pub(crate) struct PerChannel {
    /// Makes another instance of the same plugin (unprepared).
    make: fn(&str) -> Option<Box<dyn Plugin>>,
    first: Box<dyn Plugin>,
    /// One mono instance per channel past the second (made in `prepare`, off the audio thread).
    rest: Vec<Box<dyn Plugin>>,
    /// The state last given to `load_state`, for instances made after it.
    state: Option<Vec<u8>>,
    offline: bool,
    /// The linking key (two copies of one signal), sized in `prepare` for wide tracks of
    /// plugins that link (see the module docs); empty otherwise.
    link_key: Vec<Vec<f32>>,
    /// The LFE channels ([`Plugin::set_lfe_channels`]): kept out of the link.
    lfe: u64,
    /// The channels the first instance takes: two, or one for multi-mono ([`PerChannel::mono`]).
    first_width: usize,
    /// Multi-mono unlinked ([`PerChannel::unlinked`]): a plain id sets the first instance only,
    /// `c<n>:<id>` sets channel n's.
    unlinked: bool,
    /// Unlinked: each channel's stored state from the last [`pack_states`] blob given to
    /// `load_state` (index 0 the first channel's), for the instances made after it.
    states: Vec<Option<Vec<u8>>>,
}

/// The head of a multi-mono unlinked module's stored state: every channel's own state, packed by
/// [`pack_states`]. Long and unlike a plug-in's own state, so a plain state is never taken for one.
const PACK_MAGIC: &[u8] = b"FreeDAWm multi-mono states v1\0";

/// The most channels a packed state carries (more than any track has).
const PACK_MAX: usize = 64;

/// Every channel's state in one blob (`None`: that channel's instance gave none).
pub fn pack_states(states: &[Option<Vec<u8>>]) -> Vec<u8> {
    let states = states.get(..states.len().min(PACK_MAX)).unwrap_or(&[]);
    let mut out = Vec::with_capacity(PACK_MAGIC.len() + 2 + states.iter().map(|s| 4 + s.as_ref().map_or(0, Vec::len)).sum::<usize>());
    out.extend_from_slice(PACK_MAGIC);
    out.extend_from_slice(&(states.len() as u16).to_le_bytes());
    for st in states {
        match st {
            Some(b) if u32::try_from(b.len()).is_ok_and(|n| n < u32::MAX) => {
                out.extend_from_slice(&(b.len() as u32).to_le_bytes());
                out.extend_from_slice(b);
            }
            _ => out.extend_from_slice(&u32::MAX.to_le_bytes()),
        }
    }
    out
}

/// The channels' states of a [`pack_states`] blob; `None` for any other data (a plug-in's own
/// state) or a damaged blob. Hostile input: every length is checked.
pub fn unpack_states(data: &[u8]) -> Option<Vec<Option<Vec<u8>>>> {
    let rest = data.strip_prefix(PACK_MAGIC)?;
    let (n, mut rest) = rest.split_first_chunk::<2>()?;
    let n = usize::from(u16::from_le_bytes(*n));
    if n == 0 || n > PACK_MAX {
        return None;
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let (len, tail) = rest.split_first_chunk::<4>()?;
        let len = u32::from_le_bytes(*len);
        if len == u32::MAX {
            out.push(None);
            rest = tail;
            continue;
        }
        let len = usize::try_from(len).ok()?;
        let (b, tail) = (tail.get(..len)?, tail.get(len..)?);
        out.push(Some(b.to_vec()));
        rest = tail;
    }
    rest.is_empty().then_some(out)
}

/// The state a single instance takes from stored `data`: the first channel's of a packed blob
/// (a module that was multi-mono unlinked), else `data` itself.
pub fn first_state(data: &[u8]) -> Option<Vec<u8>> {
    match unpack_states(data) {
        Some(mut v) => v.first_mut().and_then(Option::take),
        None => Some(data.to_vec()),
    }
}

/// Reads every channel's state of a multi-mono unlinked module, packed ([`pack_states`]).
struct PackedReader {
    first: Box<dyn PluginStateReader>,
    rest: Vec<Option<Box<dyn PluginStateReader>>>,
}

impl PluginStateReader for PackedReader {
    fn save_state(&mut self) -> Option<Vec<u8>> {
        let first = self.first.save_state();
        let rest: Vec<Option<Vec<u8>>> = self.rest.iter_mut().map(|r| r.as_mut().and_then(|r| r.save_state())).collect();
        if first.is_none() && rest.iter().all(Option::is_none) {
            return None;
        }
        Some(pack_states(&std::iter::once(first).chain(rest).collect::<Vec<_>>()))
    }

    /// A packed blob gives each channel its own; a plain state (a preset of the plug-in's own)
    /// goes to the first channel, as a plain parameter id does.
    fn load_state(&mut self, data: &[u8]) -> bool {
        let Some(parts) = unpack_states(data) else { return self.first.load_state(data) };
        let mut ok = match parts.first() {
            Some(Some(b)) => self.first.load_state(b),
            _ => true,
        };
        for (r, part) in self.rest.iter_mut().zip(parts.iter().skip(1)) {
            if let (Some(r), Some(b)) = (r.as_mut(), part) {
                ok &= r.load_state(b);
            }
        }
        ok
    }

    fn load_preset(&mut self, kind: u32, location: Option<&str>, load_key: Option<&str>) -> bool {
        self.first.load_preset(kind, location, load_key)
    }

    fn crashed(&self) -> bool {
        self.first.crashed() || self.rest.iter().flatten().any(|r| r.crashed())
    }
}

/// A per-channel parameter id (`c1:gain`) split into its channel (1 or more) and the plugin's
/// own id, as the model writes them (`soundcraft_model::channel_param_id`).
fn split_channel(id: &str) -> Option<(usize, &str)> {
    let (n, param) = id.strip_prefix('c')?.split_once(':')?;
    if n.is_empty() || n.len() > 3 || !n.bytes().all(|b| b.is_ascii_digit()) || param.is_empty() {
        return None;
    }
    let n: usize = n.parse().ok()?;
    (n >= 1).then_some((n, param))
}

/// The parameter that links detection across every channel.
const SUR_LINK: &str = "surLink";

impl PerChannel {
    pub(crate) fn new(first: Box<dyn Plugin>, make: fn(&str) -> Option<Box<dyn Plugin>>) -> Self {
        PerChannel {
            make,
            first,
            rest: Vec::new(),
            state: None,
            offline: false,
            link_key: Vec::new(),
            lfe: 0,
            first_width: 2,
            unlinked: false,
            states: Vec::new(),
        }
    }

    /// Multi-mono: one mono instance per channel, the first included, every one taking the
    /// first's parameters (linked).
    pub(crate) fn mono(first: Box<dyn Plugin>, make: fn(&str) -> Option<Box<dyn Plugin>>) -> Self {
        PerChannel { first_width: 1, ..PerChannel::new(first, make) }
    }

    /// Multi-mono unlinked: one mono instance per channel, each with its own parameters (the
    /// first channel's under the plain ids, channel n's under `c<n>:<id>`).
    pub(crate) fn unlinked(first: Box<dyn Plugin>, make: fn(&str) -> Option<Box<dyn Plugin>>) -> Self {
        PerChannel { unlinked: true, ..PerChannel::mono(first, make) }
    }

    /// The instance a per-channel id is for (`None`: not one, or no such channel), with the
    /// plugin's own id.
    fn channel_of<'a>(&mut self, id: &'a str) -> Option<(&mut Box<dyn Plugin>, &'a str)> {
        let (n, param) = split_channel(id)?;
        let i = n.checked_sub(self.first_width)?;
        self.rest.get_mut(i).map(|p| (p, param))
    }

    fn all(&mut self) -> impl Iterator<Item = &mut Box<dyn Plugin>> {
        std::iter::once(&mut self.first).chain(self.rest.iter_mut())
    }
}

impl Plugin for PerChannel {
    fn info(&self) -> &'static PluginInfo {
        self.first.info()
    }

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.first.prepare(sample_rate, max_block, channels.min(self.first_width));
        let want = channels.saturating_sub(self.first_width);
        self.rest.truncate(want);
        let info = self.first.info();
        while self.rest.len() < want {
            let Some(mut p) = (self.make)(info.id) else { break };
            p.set_offline(self.offline);
            // Unlinked: the channel's own stored state, else the first's.
            let own = self.states.get(self.rest.len() + self.first_width).and_then(Option::as_ref);
            if let Some(st) = own.or(self.state.as_ref()) {
                p.load_state(st);
            }
            // The parameters before the first prepare, as the host sets them on a new instance:
            // nothing glides in from the defaults.
            for prm in info.params {
                if let Some(v) = self.first.param(prm.id) {
                    p.set_param(prm.id, v);
                }
            }
            self.rest.push(p);
        }
        for p in &mut self.rest {
            p.prepare(sample_rate, max_block, 1);
        }
        let links = info.sidechain && !self.rest.is_empty() && info.params.iter().any(|p| p.id == SUR_LINK);
        self.link_key = if links { vec![vec![0.0; max_block]; 2] } else { Vec::new() };
    }

    fn reset(&mut self) {
        self.all().for_each(|p| p.reset());
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        if split_channel(id).is_some() {
            // Another channel's value: its instance only, and only while unlinked.
            return self.unlinked && self.channel_of(id).is_some_and(|(p, param)| p.set_param(param, value));
        }
        if self.unlinked {
            return self.first.set_param(id, value);
        }
        let known = self.first.set_param(id, value);
        for p in &mut self.rest {
            p.set_param(id, value);
        }
        known
    }

    fn param(&self, id: &str) -> Option<f32> {
        match split_channel(id) {
            Some((n, param)) if self.unlinked => self.rest.get(n.checked_sub(self.first_width)?)?.param(param),
            Some(_) => None,
            None => self.first.param(id),
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.process_with_sidechain(io, frames, None);
    }

    fn process_with_sidechain(&mut self, io: &mut [Vec<f32>], frames: usize, key: Option<&[Vec<f32>]>) {
        let linked = key.is_none()
            && !self.link_key.is_empty()
            && self.first.param(SUR_LINK).is_some_and(|v| v > 0.5)
            && self.link_key.iter().all(|k| k.len() >= frames)
            && io.iter().all(|c| c.len() >= frames);
        let lfe = self.lfe;
        let is_lfe = move |c: usize| c < 64 && lfe & (1u64 << c) != 0;
        if linked {
            // each sample, the loudest channel's value (signed, so the key keeps its spectrum),
            // the LFE left out
            for i in 0..frames {
                let v = io
                    .iter()
                    .enumerate()
                    .filter(|(c, _)| !is_lfe(*c))
                    .map(|(_, c)| c[i])
                    .filter(|v| v.is_finite())
                    .fold(0.0f32, |m, v| if v.abs() > m.abs() { v } else { m });
                for k in &mut self.link_key {
                    k[i] = v;
                }
            }
        }
        let key = if linked { Some(self.link_key.as_slice()) } else { key };
        let fw = self.first_width;
        let n = io.len().min(fw);
        let (head, tail) = io.split_at_mut(n);
        self.first.process_with_sidechain(head, frames, key);
        for (i, (p, c)) in self.rest.iter_mut().zip(tail.iter_mut()).enumerate() {
            // the LFE is not turned down by the others' link: it keys from itself
            let k = if linked && is_lfe(i + fw) { None } else { key };
            p.process_with_sidechain(std::slice::from_mut(c), frames, k);
        }
    }

    fn latency(&self) -> usize {
        self.rest.iter().map(|p| p.latency()).fold(self.first.latency(), usize::max)
    }

    fn tail_samples(&self) -> usize {
        self.rest.iter().map(|p| p.tail_samples()).fold(self.first.tail_samples(), usize::max)
    }

    fn gain_reduction_db(&self) -> f32 {
        self.first.gain_reduction_db()
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        self.first.readouts(out);
    }

    fn analysis(&self) -> Option<Arc<analysis::Analysis>> {
        self.first.analysis()
    }

    fn set_transport(&mut self, transport: &Transport) {
        self.all().for_each(|p| p.set_transport(transport));
    }

    fn follows_song_key(&self) -> bool {
        self.first.follows_song_key()
    }

    fn live_monitoring(&self) -> bool {
        self.first.live_monitoring()
    }

    fn set_song_keys(&mut self, keys: &[crate::SongKey]) {
        self.all().for_each(|p| p.set_song_keys(keys));
    }

    fn set_offline(&mut self, offline: bool) {
        self.offline = offline;
        self.all().for_each(|p| p.set_offline(offline));
    }

    fn set_lfe_channels(&mut self, mask: u64) {
        self.lfe = mask;
        self.first.set_lfe_channels(mask & if self.first_width == 1 { 0b1 } else { 0b11 });
    }

    fn set_param_automated(&mut self, id: &str, automated: bool) {
        if split_channel(id).is_some() {
            if self.unlinked
                && let Some((p, param)) = self.channel_of(id)
            {
                p.set_param_automated(param, automated);
            }
        } else if self.unlinked {
            self.first.set_param_automated(id, automated);
        } else {
            self.all().for_each(|p| p.set_param_automated(id, automated));
        }
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.first.note_on(offset, note, velocity);
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.first.note_off(offset, note);
    }

    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        self.first.midi(offset, msg);
    }

    fn midi_raw(&mut self, offset: usize, msg: [u8; 3]) {
        self.first.midi_raw(offset, msg);
    }

    fn sysex(&mut self, offset: usize, data: &[u8]) {
        self.first.sysex(offset, data);
    }

    fn all_notes_off(&mut self) {
        self.first.all_notes_off();
    }

    fn midi_out(&mut self, out: &mut MidiOut) {
        self.first.midi_out(out);
    }

    fn set_sample(&mut self, sample: Option<Arc<dyn SampleSource>>) -> bool {
        self.first.set_sample(sample)
    }

    fn set_asset(&mut self, key: &str, asset: Option<Arc<dyn SampleSource>>) -> bool {
        for p in &mut self.rest {
            p.set_asset(key, asset.clone());
        }
        self.first.set_asset(key, asset)
    }

    fn set_link(&mut self, link: Option<Arc<LinkSlab>>, member: usize) {
        self.first.set_link(link, member);
    }

    fn action(&mut self, name: &str) -> bool {
        for p in &mut self.rest {
            p.action(name);
        }
        self.first.action(name)
    }

    fn take_released_asset(&mut self) -> Option<Arc<dyn SampleSource>> {
        self.first.take_released_asset().or_else(|| self.rest.iter_mut().find_map(|p| p.take_released_asset()))
    }

    fn save_state(&mut self) -> Option<Vec<u8>> {
        if !self.unlinked || self.rest.is_empty() {
            return self.first.save_state();
        }
        let all: Vec<Option<Vec<u8>>> = self.all().map(|p| p.save_state()).collect();
        if all.iter().all(Option::is_none) { None } else { Some(pack_states(&all)) }
    }

    fn set_prepared_state(&mut self, data: &[f64]) -> bool {
        for p in &mut self.rest {
            p.set_prepared_state(data);
        }
        self.first.set_prepared_state(data)
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        // A packed blob (saved while unlinked): unlinked, each channel takes its own; linked,
        // every channel takes the first's.
        if let Some(parts) = unpack_states(data) {
            let first = parts.first().cloned().flatten();
            let ok = first.as_ref().is_none_or(|b| self.first.load_state(b));
            if !ok {
                return false;
            }
            let fw = self.first_width;
            for (i, p) in self.rest.iter_mut().enumerate() {
                let own = if self.unlinked { parts.get(i + fw).and_then(Option::as_ref) } else { None };
                if let Some(b) = own.or(first.as_ref()) {
                    p.load_state(b);
                }
            }
            self.state = first;
            self.states = if self.unlinked { parts } else { Vec::new() };
            return true;
        }
        let ok = self.first.load_state(data);
        if ok {
            for p in &mut self.rest {
                p.load_state(data);
            }
            self.state = Some(data.to_vec());
            self.states.clear();
        }
        ok
    }

    fn open_editor(&mut self) -> Result<(), String> {
        self.first.open_editor()
    }

    fn close_editor(&mut self) {
        self.first.close_editor();
    }

    fn editor(&mut self) -> Option<Box<dyn PluginEditor>> {
        self.first.editor()
    }

    /// Unlinked with more than one instance: every channel's state, packed ([`pack_states`]).
    fn state_reader(&mut self) -> Option<Box<dyn PluginStateReader>> {
        let first = self.first.state_reader()?;
        if !self.unlinked || self.rest.is_empty() {
            return Some(first);
        }
        let rest = self.rest.iter_mut().map(|p| p.state_reader()).collect();
        Some(Box::new(PackedReader { first, rest }))
    }
}

#[cfg(test)]
mod tests {
    use super::super::instantiate;

    fn sine(frames: usize, phase: f32) -> Vec<f32> {
        (0..frames).map(|i| 0.5 * (i as f32 * 0.031 + phase).sin()).collect()
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().map(|v| v.abs()).fold(0.0, f32::max)
    }

    /// Bug 2 of the built-in effects parity list: on a 5.1 track the centre, LFE and surrounds
    /// passed dry. Each now goes through its own mono instance, as on a mono track.
    #[test]
    fn channels_past_the_second_are_processed_like_a_mono_track() {
        let frames = 512;
        let mut wide = instantiate("fs_sat").unwrap();
        let mut mono = instantiate("fs_sat").unwrap();
        for p in [&mut wide, &mut mono] {
            assert!(p.set_param("output", 1.0));
            assert!(p.set_param("drive", 0.9));
        }
        wide.prepare(48_000.0, frames, 6);
        mono.prepare(48_000.0, frames, 1);
        let dry: Vec<Vec<f32>> = (0..6).map(|c| sine(frames, c as f32)).collect();
        let mut io = dry.clone();
        let mut m = vec![dry[4].clone()];
        for _ in 0..4 {
            io.clone_from(&dry);
            wide.process(&mut io, frames);
            m[0].clone_from(&dry[4]);
            mono.process(&mut m, frames);
        }
        for (c, (out, inp)) in io.iter().zip(&dry).enumerate() {
            let diff = out.iter().zip(inp).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
            assert!(diff > 0.05, "channel {c} passed dry");
        }
        assert_eq!(io[4], m[0], "a surround channel sounds as on a mono track");
        // A parameter set later reaches every instance.
        assert!(wide.set_param("output", 0.0));
        for _ in 0..8 {
            io.clone_from(&dry);
            wide.process(&mut io, frames);
        }
        assert!(peak(&io[5]) < peak(&dry[5]));
    }

    /// The parity list's linked detection: with Link All Channels on, a loud surround channel
    /// turns Comp down on every channel of a 5.1 track; off, the front pair is left alone.
    #[test]
    fn comp_links_detection_across_every_channel_when_asked() {
        let frames = 512;
        let front = |linked: bool| {
            let mut p = instantiate("fs_comp").unwrap();
            assert!(p.set_param("amount", 0.6)); // threshold -30 dB
            assert!(p.set_param("ratio", 0.9));
            assert!(p.set_param("surLink", if linked { 1.0 } else { 0.0 }));
            p.prepare(48_000.0, frames, 6);
            let mut io: Vec<Vec<f32>> = Vec::new();
            for _ in 0..40 {
                io = (0..6).map(|c| if c == 4 { sine(frames, 0.0) } else { sine(frames, 0.0).iter().map(|v| v * 0.02).collect() }).collect();
                p.process(&mut io, frames);
            }
            (peak(&io[0]), peak(&io[3]))
        };
        let (alone, lfe_alone) = front(false);
        let (linked, lfe_linked) = front(true);
        assert!((alone - 0.01).abs() < 0.002, "a quiet front pair alone is left alone: {alone}");
        assert!(linked < 0.5 * alone, "linked, the loud surround turns the front down: {linked} vs {alone}");
        assert!(lfe_linked < 0.5 * lfe_alone, "no layout given: every channel is linked");
    }

    /// The owner's rule: told the track is 5.1 (L R C LFE Ls Rs), Link All Channels leaves the LFE
    /// out. A loud LFE does not turn the others down, and a loud surround does not turn the LFE
    /// down; the other channels still link.
    #[test]
    fn comp_link_leaves_the_lfe_of_a_5_1_track_out() {
        let frames = 512;
        // (peak of L, of LFE, of Rs) after 40 blocks; `loud` is the channel at full level
        let run = |loud: usize, lfe_mask: u64| {
            let mut p = instantiate("fs_comp").unwrap();
            assert!(p.set_param("amount", 0.6));
            assert!(p.set_param("ratio", 0.9));
            assert!(p.set_param("surLink", 1.0));
            p.set_lfe_channels(lfe_mask);
            p.prepare(48_000.0, frames, 6);
            let mut io: Vec<Vec<f32>> = Vec::new();
            for _ in 0..40 {
                io = (0..6).map(|c| if c == loud { sine(frames, 0.0) } else { sine(frames, 0.0).iter().map(|v| v * 0.02).collect() }).collect();
                p.process(&mut io, frames);
            }
            (peak(&io[0]), peak(&io[3]), peak(&io[5]))
        };
        const LFE_5_1: u64 = 1 << 3;
        // A loud LFE: the front and surrounds are left alone.
        let (l, _, rs) = run(3, LFE_5_1);
        assert!((l - 0.01).abs() < 0.002 && (rs - 0.01).abs() < 0.002, "a loud LFE turns nothing else down: {l} {rs}");
        // Without the layout it did (the old behaviour, still what an unknown layout gets).
        let (l_old, _, _) = run(3, 0);
        assert!(l_old < 0.5 * l, "{l_old} vs {l}");
        // A loud surround: the front still follows it, the quiet LFE does not.
        let (l, lfe, _) = run(4, LFE_5_1);
        assert!(l < 0.005, "the others still link: {l}");
        assert!((lfe - 0.01).abs() < 0.002, "the LFE is not turned down by the others: {lfe}");
    }

    /// Multi-mono unlinked: the plain ids set the first channel, `c1:` ids the second, so two
    /// identical channels come out different; linked, a `c1:` id changes nothing.
    #[test]
    fn unlinked_multi_mono_gives_each_channel_its_own_values() {
        let frames = 512;
        let run = |unlinked: bool| {
            let first = instantiate("fs_sat").unwrap();
            let mut p = if unlinked { super::super::multi_mono_unlinked(first, instantiate) } else { super::super::multi_mono(first, instantiate) };
            assert!(p.set_param("drive", 0.5));
            assert!(p.set_param("output", 1.0));
            p.prepare(48_000.0, frames, 2);
            assert_eq!(p.set_param("c1:output", 0.0), unlinked, "a c1 id is taken only unlinked");
            assert_eq!(p.param("c1:output"), unlinked.then_some(0.0));
            let mut io = vec![sine(frames, 0.0); 2];
            for _ in 0..8 {
                io = vec![sine(frames, 0.0); 2];
                p.process(&mut io, frames);
            }
            (peak(&io[0]), peak(&io[1]))
        };
        let (a, b) = run(false);
        assert_eq!(a, b, "linked: the same on both");
        let (a2, b2) = run(true);
        assert_eq!(a2, a, "the first channel as linked");
        assert!(b2 < 0.5 * a2, "the second channel turned down on its own: {b2} vs {a2}");
    }

    /// A plug-in whose whole state is one byte, read through its state reader.
    struct Mem(std::sync::Arc<std::sync::atomic::AtomicU8>);

    impl crate::Plugin for Mem {
        fn info(&self) -> &'static crate::PluginInfo {
            crate::plugin_info("gain").unwrap()
        }
        fn prepare(&mut self, _: f32, _: usize, _: usize) {}
        fn reset(&mut self) {}
        fn set_param(&mut self, _: &str, _: f32) -> bool {
            false
        }
        fn param(&self, _: &str) -> Option<f32> {
            None
        }
        fn process(&mut self, _: &mut [Vec<f32>], _: usize) {}
        fn save_state(&mut self) -> Option<Vec<u8>> {
            Some(vec![self.0.load(std::sync::atomic::Ordering::Relaxed)])
        }
        fn load_state(&mut self, data: &[u8]) -> bool {
            let [b] = data else { return false };
            self.0.store(*b, std::sync::atomic::Ordering::Relaxed);
            true
        }
        fn state_reader(&mut self) -> Option<Box<dyn crate::PluginStateReader>> {
            Some(Box::new(Mem(std::sync::Arc::clone(&self.0))))
        }
    }

    impl crate::PluginStateReader for Mem {
        fn save_state(&mut self) -> Option<Vec<u8>> {
            Some(vec![self.0.load(std::sync::atomic::Ordering::Relaxed)])
        }
        fn load_state(&mut self, data: &[u8]) -> bool {
            let [b] = data else { return false };
            self.0.store(*b, std::sync::atomic::Ordering::Relaxed);
            true
        }
    }

    fn mem(_: &str) -> Option<Box<dyn crate::Plugin>> {
        Some(Box::new(Mem(Default::default())))
    }

    /// Multi-mono unlinked: each channel's hosted state is saved and comes back on its own
    /// channel; linked (or a single instance), the first channel's is taken.
    #[test]
    fn unlinked_multi_mono_saves_and_restores_each_channels_state() {
        use super::{first_state, pack_states, unpack_states};
        let made = |unlinked: bool, state: Option<&[u8]>| {
            let first = mem("").unwrap();
            let mut p = if unlinked { super::super::multi_mono_unlinked(first, mem) } else { super::super::multi_mono(first, mem) };
            if let Some(st) = state {
                assert!(p.load_state(st));
            }
            p.prepare(48_000.0, 64, 3);
            p
        };
        let packed = pack_states(&[Some(vec![1]), Some(vec![2]), Some(vec![3])]);
        let mut p = made(true, Some(&packed));
        let mut reader = p.state_reader().unwrap();
        assert_eq!(reader.save_state(), Some(packed.clone()), "each channel's own state, saved");
        assert_eq!(p.save_state(), Some(packed.clone()));
        // Into the running instance (a preset in place): packed goes channel by channel, a plain
        // state to the first channel only.
        assert!(reader.load_state(&pack_states(&[Some(vec![7]), None, Some(vec![9])])));
        assert_eq!(unpack_states(&reader.save_state().unwrap()).unwrap(), [Some(vec![7]), Some(vec![2]), Some(vec![9])]);
        assert!(reader.load_state(&[5]));
        assert_eq!(unpack_states(&reader.save_state().unwrap()).unwrap(), [Some(vec![5]), Some(vec![2]), Some(vec![9])]);
        // Linked, every channel takes the first's; its reader is the first's.
        let mut linked = made(false, Some(&packed));
        assert_eq!(linked.state_reader().unwrap().save_state(), Some(vec![1]));
        // A plain state, unlinked: every channel starts from it, as before.
        let mut plain = made(true, Some(&[4]));
        let saved = plain.state_reader().unwrap().save_state().unwrap();
        assert_eq!(unpack_states(&saved).unwrap(), [Some(vec![4]), Some(vec![4]), Some(vec![4])]);
        // A single instance takes the first channel's; anything else passes through.
        assert_eq!(first_state(&packed), Some(vec![1]));
        assert_eq!(first_state(b"plain"), Some(b"plain".to_vec()));
        assert_eq!(first_state(&pack_states(&[None, Some(vec![2])])), None);
        let mut cut = packed.clone();
        cut.pop();
        assert!(unpack_states(&cut).is_none(), "truncated");
        let mut long = packed.clone();
        long.push(0);
        assert!(unpack_states(&long).is_none(), "trailing bytes");
        assert!(unpack_states(b"plain").is_none());
    }

    #[test]
    fn every_stereo_only_plugin_runs_on_twelve_channels_and_on_stereo() {
        for info in super::super::REGISTRY.iter().filter(|i| super::wraps(i.id)) {
            let mut p = instantiate(info.id).unwrap();
            assert_eq!(p.info().id, info.id);
            p.prepare(48_000.0, 256, 12);
            let mut io: Vec<Vec<f32>> = (0..12).map(|c| sine(256, c as f32)).collect();
            p.process(&mut io, 256);
            assert!(io.iter().flatten().all(|x| x.is_finite()), "{}", info.id);
            p.prepare(48_000.0, 256, 2);
            let mut io: Vec<Vec<f32>> = (0..2).map(|c| sine(256, c as f32)).collect();
            p.process(&mut io, 256);
        }
    }
}
