//! Sample-accurate parameter changes for built-in plugins.
//!
//! The mixer sends a lane that moves inside a block as timed changes ([`Plugin::set_param_at`]),
//! every 64 samples where it moves. A built-in plugin listed in [`takes`] comes wrapped in
//! [`Timed`]: it keeps those changes in a fixed-size list and, when a block has any, runs the
//! plugin in pieces split at their offsets, setting each value (with [`Plugin::set_param`])
//! just before the piece that starts there. The plugin's own smoothing then glides from one
//! value to the next as it does between blocks, so a ramp drawn inside a block is followed
//! within a block instead of jumping once per block. A block without timed changes goes straight
//! to the plugin, untouched.
//!
//! The pieces are copied through buffers made in `prepare` (and the key too, for a plugin with a
//! key input): nothing is allocated while processing. Meters and readouts are the last piece's.

use crate::{LinkSlab, MidiOut, Plugin, PluginEditor, PluginInfo, PluginStateReader, SampleSource, Transport, analysis};
use std::sync::Arc;

/// The built-in plugins that take timed changes: the most automated ones (the parity list's
/// EQ, filter, gain and compressor first). Others keep changing once per block.
const TAKES: &[&str] = &["gain", "trim", "channel_tool", "fs_cleanq", "fs_filter", "fs_comp"];

/// Whether [`super::instantiate`] wraps plugin `id` in [`Timed`].
pub(crate) fn takes(id: &str) -> bool {
    TAKES.contains(&id)
}

/// Most timed changes kept for one block; past that `set_param_at` declines (the host then
/// changes that parameter once per block, as before).
const CAPACITY: usize = 256;

#[derive(Clone, Copy)]
struct Change {
    offset: usize,
    id: &'static str,
    value: f32,
}

/// A plugin that takes timed parameter changes by running in pieces (see the module docs).
pub(crate) struct Timed {
    inner: Box<dyn Plugin>,
    changes: [Change; CAPACITY],
    len: usize,
    /// One buffer per prepared channel, `max_block` long, for a piece of the block.
    piece: Vec<Vec<f32>>,
    /// The same for the key input.
    key_piece: Vec<Vec<f32>>,
}

impl Timed {
    pub(crate) fn new(inner: Box<dyn Plugin>) -> Self {
        Timed { inner, changes: [Change { offset: 0, id: "", value: 0.0 }; CAPACITY], len: 0, piece: Vec::new(), key_piece: Vec::new() }
    }

    /// Applies (in the order they came) every kept change at or before `at` and marks it done.
    fn apply_up_to(&mut self, at: usize) {
        for c in self.changes.iter_mut().take(self.len) {
            if !c.id.is_empty() && c.offset <= at {
                self.inner.set_param(c.id, c.value);
                c.id = "";
            }
        }
    }

    /// The earliest offset of a change not yet applied, after `at`.
    fn next_after(&self, at: usize) -> Option<usize> {
        self.changes.iter().take(self.len).filter(|c| !c.id.is_empty() && c.offset > at).map(|c| c.offset).min()
    }

    fn run(&mut self, io: &mut [Vec<f32>], frames: usize, key: Option<&[Vec<f32>]>) {
        if self.len == 0 {
            self.inner.process_with_sidechain(io, frames, key);
            return;
        }
        let frames = io.iter().map(Vec::len).fold(frames, usize::min);
        let nc = io.len().min(self.piece.len());
        let fits = self.piece.iter().take(nc).all(|b| b.len() >= frames)
            && key.is_none_or(|k| k.len() <= self.key_piece.len() && self.key_piece.iter().all(|b| b.len() >= frames));
        if !fits || nc < io.len() {
            // A block larger than prepared (never from the mixer): the changes land at its start.
            self.apply_up_to(usize::MAX);
            self.len = 0;
            self.inner.process_with_sidechain(io, frames, key);
            return;
        }
        let mut at = 0;
        while at < frames {
            self.apply_up_to(at);
            let end = self.next_after(at).map_or(frames, |n| n.min(frames));
            if at == 0 && end == frames {
                self.inner.process_with_sidechain(io, frames, key);
                break;
            }
            let len = end - at;
            for (buf, ch) in self.piece.iter_mut().zip(io.iter()) {
                buf[..len].copy_from_slice(&ch[at..end]);
            }
            let k = match key {
                Some(k) => {
                    for (buf, ch) in self.key_piece.iter_mut().zip(k.iter()) {
                        let n = ch.len().min(end).saturating_sub(at);
                        buf[..n].copy_from_slice(&ch[at..at + n]);
                        buf[n..len].fill(0.0);
                    }
                    Some(&self.key_piece[..k.len()])
                }
                None => None,
            };
            self.inner.process_with_sidechain(&mut self.piece[..nc], len, k);
            for (buf, ch) in self.piece.iter().zip(io.iter_mut()) {
                ch[at..end].copy_from_slice(&buf[..len]);
            }
            at = end;
        }
        // Anything at or past the block's end takes effect for the next block.
        self.apply_up_to(usize::MAX);
        self.len = 0;
    }
}

impl Plugin for Timed {
    fn info(&self) -> &'static PluginInfo {
        self.inner.info()
    }

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        self.inner.prepare(sample_rate, max_block, channels);
        self.piece = vec![vec![0.0; max_block]; channels];
        self.key_piece = if self.inner.info().sidechain { vec![vec![0.0; max_block]; channels.max(2)] } else { Vec::new() };
        self.len = 0;
    }

    fn reset(&mut self) {
        self.inner.reset();
    }

    fn set_param(&mut self, id: &str, value: f32) -> bool {
        self.inner.set_param(id, value)
    }

    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }

    fn set_param_at(&mut self, offset: usize, id: &str, value: f32) -> bool {
        // The id as the plugin's own `'static` name, so nothing is copied.
        let Some(p) = self.inner.info().param(id) else { return false };
        let Some(slot) = self.changes.get_mut(self.len) else { return false };
        *slot = Change { offset, id: p.id, value };
        self.len += 1;
        true
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.run(io, frames, None);
    }

    fn process_with_sidechain(&mut self, io: &mut [Vec<f32>], frames: usize, key: Option<&[Vec<f32>]>) {
        self.run(io, frames, key);
    }

    fn latency(&self) -> usize {
        self.inner.latency()
    }

    fn tail_samples(&self) -> usize {
        self.inner.tail_samples()
    }

    fn gain_reduction_db(&self) -> f32 {
        self.inner.gain_reduction_db()
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        self.inner.readouts(out);
    }

    fn analysis(&self) -> Option<Arc<analysis::Analysis>> {
        self.inner.analysis()
    }

    fn set_transport(&mut self, transport: &Transport) {
        self.inner.set_transport(transport);
    }

    fn follows_song_key(&self) -> bool {
        self.inner.follows_song_key()
    }

    fn live_monitoring(&self) -> bool {
        self.inner.live_monitoring()
    }

    fn set_song_keys(&mut self, keys: &[crate::SongKey]) {
        self.inner.set_song_keys(keys);
    }

    fn set_offline(&mut self, offline: bool) {
        self.inner.set_offline(offline);
    }

    fn set_lfe_channels(&mut self, mask: u64) {
        self.inner.set_lfe_channels(mask);
    }

    fn set_param_automated(&mut self, id: &str, automated: bool) {
        self.inner.set_param_automated(id, automated);
    }

    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.inner.note_on(offset, note, velocity);
    }

    fn note_off(&mut self, offset: usize, note: u8) {
        self.inner.note_off(offset, note);
    }

    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        self.inner.midi(offset, msg);
    }

    fn midi_raw(&mut self, offset: usize, msg: [u8; 3]) {
        self.inner.midi_raw(offset, msg);
    }

    fn sysex(&mut self, offset: usize, data: &[u8]) {
        self.inner.sysex(offset, data);
    }

    fn all_notes_off(&mut self) {
        self.inner.all_notes_off();
    }

    fn midi_out(&mut self, out: &mut MidiOut) {
        self.inner.midi_out(out);
    }

    fn set_sample(&mut self, sample: Option<Arc<dyn SampleSource>>) -> bool {
        self.inner.set_sample(sample)
    }

    fn set_asset(&mut self, key: &str, asset: Option<Arc<dyn SampleSource>>) -> bool {
        self.inner.set_asset(key, asset)
    }

    fn set_link(&mut self, link: Option<Arc<LinkSlab>>, member: usize) {
        self.inner.set_link(link, member);
    }

    fn action(&mut self, name: &str) -> bool {
        self.inner.action(name)
    }

    fn take_released_asset(&mut self) -> Option<Arc<dyn SampleSource>> {
        self.inner.take_released_asset()
    }

    fn save_state(&mut self) -> Option<Vec<u8>> {
        self.inner.save_state()
    }

    fn set_prepared_state(&mut self, data: &[f64]) -> bool {
        self.inner.set_prepared_state(data)
    }

    fn load_state(&mut self, data: &[u8]) -> bool {
        self.inner.load_state(data)
    }

    fn open_editor(&mut self) -> Result<(), String> {
        self.inner.open_editor()
    }

    fn close_editor(&mut self) {
        self.inner.close_editor();
    }

    fn editor(&mut self) -> Option<Box<dyn PluginEditor>> {
        self.inner.editor()
    }

    fn state_reader(&mut self) -> Option<Box<dyn PluginStateReader>> {
        self.inner.state_reader()
    }
}

#[cfg(test)]
mod tests {
    use super::super::instantiate;

    fn dc(frames: usize) -> Vec<Vec<f32>> {
        vec![vec![1.0; frames]; 2]
    }

    /// A gain change half-way through a block takes effect there, not at the block's start or
    /// the next block's.
    #[test]
    fn a_gain_change_inside_a_block_lands_where_it_was_sent() {
        let frames = 1024;
        let mut g = instantiate("gain").unwrap();
        g.prepare(48_000.0, frames, 2);
        assert!(g.set_param("gain", 0.0));
        let mut io = dc(frames);
        g.process(&mut io, frames);
        assert!(g.set_param_at(512, "gain", -60.0), "gain takes timed changes");
        let mut io = dc(frames);
        g.process(&mut io, frames);
        // unity up to the change, then the 20 ms smoother heads down
        assert!((io[0][511] - 1.0).abs() < 1e-4, "{}", io[0][511]);
        assert!(io[0][1023] < 0.9, "{}", io[0][1023]);
        assert_eq!(io[0], io[1]);
        // the plugin holds the new value afterwards
        assert_eq!(g.param("gain"), Some(-60.0));
    }

    /// Without timed changes the wrapper is the plugin: the same samples.
    #[test]
    fn no_timed_changes_sounds_the_same() {
        let frames = 512;
        for id in ["fs_comp", "fs_cleanq", "fs_filter", "gain"] {
            let mut a = instantiate(id).unwrap();
            let mut b = super::super::instantiate_one(id).unwrap();
            a.prepare(48_000.0, frames, 2);
            b.prepare(48_000.0, frames, 2);
            let input: Vec<Vec<f32>> = (0..2).map(|c| (0..frames).map(|i| 0.5 * (i as f32 * 0.05 + c as f32).sin()).collect()).collect();
            for _ in 0..4 {
                let mut x = input.clone();
                let mut y = input.clone();
                a.process(&mut x, frames);
                b.process(&mut y, frames);
                assert_eq!(x, y, "{id}");
            }
        }
    }

    /// The parity list's first built-ins take timed changes; an unknown id is declined, and the
    /// list declines once full.
    #[test]
    fn the_most_automated_built_ins_take_timed_changes() {
        for (id, param) in [("fs_cleanq", "b1Gain"), ("fs_filter", "cutoff"), ("fs_comp", "amount"), ("trim", "gain")] {
            let mut p = instantiate(id).unwrap();
            p.prepare(48_000.0, 256, 2);
            let pid = p.info().params.iter().find(|q| q.id == param).map_or(p.info().params[0].id, |q| q.id);
            assert!(p.set_param_at(64, pid, p.info().params.iter().find(|q| q.id == pid).unwrap().default), "{id}");
            assert!(!p.set_param_at(64, "no such param", 0.0), "{id}");
            let mut io = dc(256);
            p.process(&mut io, 256);
            assert!(io.iter().flatten().all(|v| v.is_finite()), "{id}");
        }
        let mut g = instantiate("gain").unwrap();
        g.prepare(48_000.0, 256, 2);
        let taken = (0..400).filter(|i| g.set_param_at(i % 256, "gain", -1.0)).count();
        assert_eq!(taken, super::CAPACITY);
        let mut io = dc(256);
        g.process(&mut io, 256);
        // the list is emptied by a block
        assert!(g.set_param_at(0, "gain", 0.0));
        // a plugin not listed keeps the default (declines)
        let mut s = instantiate("fs_sat").unwrap();
        s.prepare(48_000.0, 256, 2);
        assert!(!s.set_param_at(64, "drive", 0.5));
    }

    /// A keyed compressor split into pieces still hears its key.
    #[test]
    fn a_keyed_comp_in_pieces_still_hears_its_key() {
        let frames = 1024;
        let mut c = instantiate("fs_comp").unwrap();
        c.prepare(48_000.0, frames, 2);
        let key: Vec<Vec<f32>> = (0..2).map(|_| (0..frames).map(|i| 0.9 * (i as f32 * 0.07).sin()).collect()).collect();
        for _ in 0..20 {
            for off in (64..frames).step_by(64) {
                c.set_param_at(off, "amount", 0.9);
            }
            let mut io: Vec<Vec<f32>> = (0..2).map(|_| (0..frames).map(|i| 0.1 * (i as f32 * 0.01).sin()).collect()).collect();
            c.process_with_sidechain(&mut io, frames, Some(&key));
            assert!(io.iter().flatten().all(|v| v.is_finite()));
        }
        assert!(c.gain_reduction_db() > 0.5, "the key turned it down: {}", c.gain_reduction_db());
    }
}
