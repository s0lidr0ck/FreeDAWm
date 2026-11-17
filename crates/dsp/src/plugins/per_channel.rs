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
}

/// The parameter that links detection across every channel.
const SUR_LINK: &str = "surLink";

impl PerChannel {
    pub(crate) fn new(first: Box<dyn Plugin>, make: fn(&str) -> Option<Box<dyn Plugin>>) -> Self {
        PerChannel { make, first, rest: Vec::new(), state: None, offline: false, link_key: Vec::new(), lfe: 0 }
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
        self.first.prepare(sample_rate, max_block, channels.min(2));
        let want = channels.saturating_sub(2);
        self.rest.truncate(want);
        let info = self.first.info();
        while self.rest.len() < want {
            let Some(mut p) = (self.make)(info.id) else { break };
            p.set_offline(self.offline);
            if let Some(st) = &self.state {
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
        let known = self.first.set_param(id, value);
        for p in &mut self.rest {
            p.set_param(id, value);
        }
        known
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.first.param(id)
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
        let n = io.len().min(2);
        let (head, tail) = io.split_at_mut(n);
        self.first.process_with_sidechain(head, frames, key);
        for (i, (p, c)) in self.rest.iter_mut().zip(tail.iter_mut()).enumerate() {
            // the LFE is not turned down by the others' link: it keys from itself
            let k = if linked && is_lfe(i + 2) { None } else { key };
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

    fn set_offline(&mut self, offline: bool) {
        self.offline = offline;
        self.all().for_each(|p| p.set_offline(offline));
    }

    fn set_lfe_channels(&mut self, mask: u64) {
        self.lfe = mask;
        self.first.set_lfe_channels(mask & 0b11);
    }

    fn set_param_automated(&mut self, id: &str, automated: bool) {
        self.all().for_each(|p| p.set_param_automated(id, automated));
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
        self.first.save_state()
    }

    fn set_prepared_state(&mut self, data: &[f64]) -> bool {
        for p in &mut self.rest {
            p.set_prepared_state(data);
        }
        self.first.set_prepared_state(data)
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        let ok = self.first.load_state(data);
        if ok {
            for p in &mut self.rest {
                p.load_state(data);
            }
            self.state = Some(data.to_vec());
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

    fn state_reader(&mut self) -> Option<Box<dyn PluginStateReader>> {
        self.first.state_reader()
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
