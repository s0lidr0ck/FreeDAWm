//! Spectrum taps: what each track sends into the mix, after its fader, mute and pan, for the
//! Spectrum editor to draw (`soundcraft-ui-egui`'s `spectrum_view`).
//!
//! One [`SpectrumTaps`] is made off the audio thread and lives for the whole process (the live
//! engines get it from `soundcraft_playback::spectrum_taps`; offline renders never have it), so
//! nothing of it is ever dropped on the audio thread. It holds [`SLOTS`] slots; the UI thread
//! claims a slot for each curve it shows ([`SpectrumTaps::claim`]), which makes the slot's ring
//! (an [`AnalysisRing`], allocated once on first claim and then reused), and switches it on only
//! while the curve is visible. The audio thread finds a strip's slot through a [`TapCache`] it
//! rescans only when the claims change, and pushes the strip's mono sum into that ring: atomic
//! stores, a stack buffer, no allocation, no lock, no panic. The FFT is the UI's.
//!
//! Cost on the audio thread: with no taps attached (offline), nothing. With taps attached and the
//! editor closed ([`SpectrumTaps::set_active`] false), one relaxed atomic load per rendered block.
//! With it open, per shown strip: one key compare and a multiply-add per channel and sample.
//!
//! The tap sits where the strip is added to its destination (the main mix, a bus or a hardware
//! output): after the inserts, the fader, the mute (a muted or solo-muted strip reads silence) and
//! the direct-output delay. The pan is taken as each source channel's total power gain into the
//! destination (`sqrt` of the sum of its squared routing gains at the block's start), so the pan
//! law's level change shows but not the side it leans to; channels are then averaged (a centred
//! mono track and a stereo track of the same audio read the same). The master tap ([`MASTER_KEY`])
//! is the main mix after the master faders, as the output meter reads it.
//!
//! For the editor's stereo scope a slot can also carry the strip's left and right as they reach
//! the destination ([`SpectrumTaps::set_stereo`]): each source channel times its routing gain into
//! the destination's first two channels (a mono destination: both sides alike). Those two rings
//! are made the first time the scope asks for the slot, and fed only while it does.

use crate::{MAX_CHANNELS, Matrix};
use soundcraft_dsp::analysis::AnalysisRing;
use soundcraft_model::TrackId;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Slots in one [`SpectrumTaps`]: curves that can be shown at once.
pub const SLOTS: usize = 256;

/// The key of the master (the main mix after the master faders).
pub const MASTER_KEY: u64 = u64::MAX;

/// A free slot's key.
const FREE: u64 = 0;

/// Frames mixed down at a time (a stack buffer: nothing allocated per block).
const CHUNK: usize = 256;

/// The slot key of track `t` (never [`FREE`] or [`MASTER_KEY`]).
pub fn track_key(t: TrackId) -> u64 {
    t.0.saturating_add(1).min(MASTER_KEY - 1)
}

/// One curve's tap.
#[derive(Default)]
pub struct TapSlot {
    /// Whose signal it takes ([`FREE`]: nobody's).
    key: AtomicU64,
    /// Fed only while on (the curve is shown).
    on: AtomicBool,
    /// Made on the UI thread at the slot's first claim; read on the audio thread with `get`.
    ring: OnceLock<AnalysisRing>,
    /// Also fed: the left and right rings (the stereo scope shows the curve).
    stereo: AtomicBool,
    /// Made on the UI thread the first time the scope asks for the slot.
    left: OnceLock<AnalysisRing>,
    right: OnceLock<AnalysisRing>,
}

/// What the audio thread feeds for one strip this block.
pub(crate) struct FeedTarget<'a> {
    mono: &'a AnalysisRing,
    stereo: Option<(&'a AnalysisRing, &'a AnalysisRing)>,
}

/// Every slot, the editor's on switch and the claims' generation. See the module docs.
pub struct SpectrumTaps {
    active: AtomicBool,
    generation: AtomicU64,
    slots: Box<[TapSlot]>,
}

impl Default for SpectrumTaps {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for SpectrumTaps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpectrumTaps").field("active", &self.is_active()).finish_non_exhaustive()
    }
}

/// Where the audio thread last found a strip's slot, and the claims' generation it was found in.
#[derive(Clone, Copy, Debug)]
pub struct TapCache {
    generation: u64,
    slot: Option<u16>,
}

impl Default for TapCache {
    fn default() -> Self {
        // A generation no taps start at: the first look always scans.
        TapCache { generation: u64::MAX, slot: None }
    }
}

impl SpectrumTaps {
    /// Allocates the slot table (call off the audio thread). The rings come with each slot's
    /// first claim.
    pub fn new() -> Self {
        SpectrumTaps { active: AtomicBool::new(false), generation: AtomicU64::new(0), slots: (0..SLOTS).map(|_| TapSlot::default()).collect() }
    }

    /// The editor is open (true) or closed: closed, no strip is fed whatever its slot says.
    pub fn set_active(&self, on: bool) {
        self.active.store(on, Ordering::Relaxed);
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    /// UI thread: the slot taking `key`'s signal, claimed now if it has none (its ring made on
    /// the slot's first claim). Starts off ([`SpectrumTaps::set_on`]). `None` when every slot is
    /// taken or `key` is [`FREE`].
    pub fn claim(&self, key: u64) -> Option<usize> {
        if key == FREE {
            return None;
        }
        if let Some(i) = self.slot_of(key) {
            return Some(i);
        }
        let i = self.slots.iter().position(|s| s.key.load(Ordering::Acquire) == FREE)?;
        let slot = self.slots.get(i)?;
        slot.ring.get_or_init(AnalysisRing::new);
        slot.on.store(false, Ordering::Relaxed);
        slot.key.store(key, Ordering::Release);
        self.generation.fetch_add(1, Ordering::AcqRel);
        Some(i)
    }

    /// UI thread: frees slot `i` for another curve.
    pub fn release(&self, i: usize) {
        if let Some(slot) = self.slots.get(i) {
            slot.on.store(false, Ordering::Relaxed);
            slot.stereo.store(false, Ordering::Relaxed);
            slot.key.store(FREE, Ordering::Release);
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// The slot taking `key`'s signal, if one is claimed.
    pub fn slot_of(&self, key: u64) -> Option<usize> {
        if key == FREE {
            return None;
        }
        self.slots.iter().position(|s| s.key.load(Ordering::Acquire) == key)
    }

    /// Feed slot `i` (true) or leave it alone (false): a hidden curve costs the audio thread
    /// nothing but the key compare.
    pub fn set_on(&self, i: usize, on: bool) {
        if let Some(slot) = self.slots.get(i) {
            slot.on.store(on, Ordering::Relaxed);
        }
    }

    /// UI thread: also feed slot `i`'s left and right rings (true; made now if they are not
    /// yet), or stop feeding them.
    pub fn set_stereo(&self, i: usize, on: bool) {
        if let Some(slot) = self.slots.get(i) {
            if on {
                slot.left.get_or_init(AnalysisRing::new);
                slot.right.get_or_init(AnalysisRing::new);
            }
            slot.stereo.store(on, Ordering::Release);
        }
    }

    pub fn is_stereo(&self, i: usize) -> bool {
        self.slots.get(i).is_some_and(|s| s.stereo.load(Ordering::Relaxed))
    }

    /// Slot `i`'s left and right rings, once the scope has asked for them.
    pub fn stereo_rings(&self, i: usize) -> Option<(&AnalysisRing, &AnalysisRing)> {
        let s = self.slots.get(i)?;
        Some((s.left.get()?, s.right.get()?))
    }

    pub fn is_on(&self, i: usize) -> bool {
        self.slots.get(i).is_some_and(|s| s.on.load(Ordering::Relaxed))
    }

    /// Slot `i`'s ring (its newest samples, mono), once claimed.
    pub fn ring(&self, i: usize) -> Option<&AnalysisRing> {
        self.slots.get(i).and_then(|s| s.ring.get())
    }

    /// Keys of the claimed slots, as (slot, key).
    pub fn claimed(&self) -> impl Iterator<Item = (usize, u64)> + '_ {
        self.slots.iter().enumerate().filter_map(|(i, s)| {
            let k = s.key.load(Ordering::Acquire);
            (k != FREE).then_some((i, k))
        })
    }

    /// Audio thread: the ring to feed `key` into this block, if its slot is claimed and on. Uses
    /// and refreshes `cache`; scans the slots only after a claim or release.
    pub(crate) fn feed_target(&self, key: u64, cache: &mut TapCache) -> Option<FeedTarget<'_>> {
        let generation = self.generation.load(Ordering::Acquire);
        if generation != cache.generation {
            cache.generation = generation;
            cache.slot = self.slot_of(key).and_then(|i| u16::try_from(i).ok());
        }
        let slot = self.slots.get(usize::from(cache.slot?))?;
        // A claim made since the generation was read: never feed another curve's ring.
        if slot.key.load(Ordering::Acquire) != key || !slot.on.load(Ordering::Relaxed) {
            return None;
        }
        let stereo = if slot.stereo.load(Ordering::Acquire) { slot.left.get().zip(slot.right.get()) } else { None };
        Some(FeedTarget { mono: slot.ring.get()?, stereo })
    }

    /// Address and size of what the audio thread writes into (the slot table and every ring
    /// made so far): nothing here moves while rendering.
    pub fn buffers(&self) -> Vec<(usize, usize)> {
        let mut v = vec![(self.slots.as_ptr() as usize, self.slots.len())];
        for s in self.slots.iter() {
            v.extend([s.ring.get(), s.left.get(), s.right.get()].into_iter().flatten().map(|r| (r as *const AnalysisRing as usize, 0)));
        }
        v
    }
}

/// Feeds `target` with frames `0..frames` of `buf` routed by `m` into a destination of `nd`
/// channels: the mono ring with each channel's power gain ([`channel_gains`]), the stereo rings
/// (when the scope asks) with its gains into the destination's first two channels.
pub(crate) fn feed(target: &FeedTarget<'_>, buf: &[Vec<f32>], m: &Matrix, nd: usize, frames: usize) {
    let g = channel_gains(m, buf.len());
    feed_mono(target.mono, buf, &g, frames);
    if let Some((l, r)) = target.stereo {
        let (mut gl, mut gr) = ([0.0f32; MAX_CHANNELS], [0.0f32; MAX_CHANNELS]);
        let right = if nd >= 2 { 1 } else { 0 };
        for (c, row) in m.iter().enumerate().take(buf.len().min(MAX_CHANNELS)) {
            if let (Some(a), Some(b)) = (gl.get_mut(c), gr.get_mut(c)) {
                *a = row.first().copied().unwrap_or(0.0);
                *b = row.get(right).copied().unwrap_or(0.0);
            }
        }
        feed_sum(l, buf, &gl, frames);
        feed_sum(r, buf, &gr, frames);
    }
}

/// Each source channel's power gain into the destination: `sqrt(Σ_d m[c][d]²)`.
pub(crate) fn channel_gains(m: &Matrix, channels: usize) -> [f32; MAX_CHANNELS] {
    let mut g = [0.0f32; MAX_CHANNELS];
    for (out, row) in g.iter_mut().zip(m.iter()).take(channels) {
        let p: f32 = row.iter().map(|x| x * x).sum();
        *out = if p.is_finite() { p.sqrt() } else { 0.0 };
    }
    g
}

/// Pushes frames `0..frames` of `buf` into `ring` as one mono signal: each channel times its
/// gain, averaged over the channels. Audio-thread safe (a stack buffer, atomic stores).
pub(crate) fn feed_mono(ring: &AnalysisRing, buf: &[Vec<f32>], gains: &[f32; MAX_CHANNELS], frames: usize) {
    let n = buf.len().min(MAX_CHANNELS);
    if n == 0 {
        return;
    }
    mix_into(ring, buf, gains, 1.0 / n as f32, frames);
}

/// Pushes `Σ_c gains[c] · buf[c]` for frames `0..frames` into `ring`.
fn feed_sum(ring: &AnalysisRing, buf: &[Vec<f32>], gains: &[f32; MAX_CHANNELS], frames: usize) {
    mix_into(ring, buf, gains, 1.0, frames);
}

fn mix_into(ring: &AnalysisRing, buf: &[Vec<f32>], gains: &[f32; MAX_CHANNELS], scale: f32, frames: usize) {
    let n = buf.len().min(MAX_CHANNELS);
    if n == 0 || frames == 0 {
        return;
    }
    let mut chunk = [0.0f32; CHUNK];
    let mut at = 0;
    while at < frames {
        let k = CHUNK.min(frames - at);
        let Some(out) = chunk.get_mut(..k) else { return };
        out.fill(0.0);
        for (ch, g) in buf.iter().zip(gains.iter()).take(n) {
            let Some(x) = ch.get(at..at + k) else { continue };
            let g = g * scale;
            for (o, v) in out.iter_mut().zip(x) {
                *o += g * v;
            }
        }
        ring.push_slice(out);
        at += k;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claims_are_found_reused_and_released() {
        let taps = SpectrumTaps::new();
        let a = taps.claim(track_key(TrackId(3))).unwrap();
        assert_eq!(taps.claim(track_key(TrackId(3))), Some(a), "the same key keeps its slot");
        let b = taps.claim(MASTER_KEY).unwrap();
        assert_ne!(a, b);
        assert!(taps.claim(FREE).is_none());
        let mut cache = TapCache::default();
        // Claimed but off: not fed.
        assert!(taps.feed_target(MASTER_KEY, &mut cache).is_none());
        taps.set_on(b, true);
        assert!(taps.feed_target(MASTER_KEY, &mut cache).is_some());
        taps.release(b);
        assert!(taps.feed_target(MASTER_KEY, &mut cache).is_none(), "a released slot is not fed");
        assert_eq!(taps.claimed().count(), 1);
    }

    #[test]
    fn every_slot_can_be_taken_and_then_none() {
        let taps = SpectrumTaps::new();
        for i in 0..SLOTS {
            assert!(taps.claim(track_key(TrackId(i as u64))).is_some());
        }
        assert!(taps.claim(track_key(TrackId(9999))).is_none());
    }

    #[test]
    fn mono_sum_averages_channels_by_gain() {
        let ring = AnalysisRing::new();
        let buf = vec![vec![1.0f32; 300], vec![0.5f32; 300]];
        let mut g = [0.0f32; MAX_CHANNELS];
        g[0] = 1.0;
        g[1] = 1.0;
        feed_mono(&ring, &buf, &g, 300);
        assert_eq!(ring.written(), 300);
        let mut out = [0.0f32; 4];
        ring.latest(&mut out);
        assert_eq!(out, [0.75; 4]);
    }
}
