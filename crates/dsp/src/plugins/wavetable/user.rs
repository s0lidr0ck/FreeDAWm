//! A user's own wavetable: up to [`MAX_WAVES`] single-cycle waves of [`WAVE_LEN`] samples made
//! from the user's audio (see [`super::import`]) and stored with the session, in the synth's
//! insert (`Insert::state`), as the bytes [`UserTable::encode`] writes: the table itself, so the
//! session does not depend on the file it came from.
//!
//! Playing one: [`prepare`] decodes the stored bytes and band-limits every wave with the same
//! half-octave mip-map as the built-in tables ([`super::tables`]), off the audio thread. The host
//! hands the result to the running synth as an asset ([`crate::Plugin::set_asset`] under
//! [`crate::PREPARED_ASSET_KEY`]); the synth only swaps the reference in (no allocation, no lock),
//! crossfades from the table it played over [`FADE_MS`], and gives the old one back through
//! [`crate::Plugin::take_released_asset`] for the host to drop off the audio thread.

use super::tables::{self, C, FRAME_LEN, LEVELS, fft, harmonics, lerp_cycle, offset, size};
use crate::SampleSource;
use std::sync::Arc;

/// Samples in each stored wave (the Serum/Vital convention).
pub const WAVE_LEN: usize = 2048;
/// Most waves in a table.
pub const MAX_WAVES: usize = 32;
/// Longest name kept, bytes of UTF-8.
pub const MAX_NAME_BYTES: usize = 64;
/// The stored form's first bytes.
const MAGIC: &[u8; 4] = b"FDWT";
const VERSION: u8 = 1;
/// Header: magic, version, waves, wave length (u16), name length.
const HEADER: usize = 4 + 1 + 1 + 2 + 1;
/// The crossfade from one table to the next when a new one arrives while playing.
pub const FADE_MS: f32 = 20.0;
/// What the synth can hold of tables it let go of before the host has taken them.
const RELEASED: usize = 4;

/// A table of single-cycle waves, as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct UserTable {
    pub name: String,
    /// 1..=[`MAX_WAVES`] waves of [`WAVE_LEN`] samples each, finite.
    pub waves: Vec<Vec<f32>>,
}

impl UserTable {
    /// A table from waves (each resampled to [`WAVE_LEN`] when it is another length; at most
    /// [`MAX_WAVES`] kept; non-finite samples become 0), the name cut to [`MAX_NAME_BYTES`].
    /// `None` when there are no waves.
    pub fn new(name: &str, waves: &[Vec<f32>]) -> Option<UserTable> {
        let waves: Vec<Vec<f32>> = waves.iter().take(MAX_WAVES).map(|w| resample_cycle(w, WAVE_LEN)).collect();
        if waves.is_empty() {
            return None;
        }
        Some(UserTable { name: cut_name(name), waves })
    }

    /// Number of waves.
    pub fn wave_count(&self) -> usize {
        self.waves.len()
    }

    /// The stored form.
    pub fn encode(&self) -> Vec<u8> {
        let name = cut_name(&self.name);
        let n = self.waves.len().min(MAX_WAVES);
        let mut out = Vec::with_capacity(HEADER + name.len() + n * WAVE_LEN * 4);
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.push(n as u8);
        out.extend_from_slice(&(WAVE_LEN as u16).to_le_bytes());
        out.push(name.len() as u8);
        out.extend_from_slice(name.as_bytes());
        for w in self.waves.iter().take(n) {
            for i in 0..WAVE_LEN {
                let v = w.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out
    }

    /// Reads the stored form. The bytes are hostile input: anything not exactly what
    /// [`UserTable::encode`] writes (another magic or version, no waves or too many, another wave
    /// length, a name that is not UTF-8, missing or extra bytes, non-finite samples) is refused
    /// with a reason.
    pub fn decode(bytes: &[u8]) -> Result<UserTable, String> {
        let head = bytes.get(..HEADER).ok_or("the wavetable is cut short")?;
        if head.get(..4) != Some(MAGIC.as_slice()) {
            return Err("not a wavetable".into());
        }
        let at = |i: usize| head.get(i).copied().unwrap_or(0);
        if at(4) != VERSION {
            return Err(format!("wavetable version {} is not one this build reads", at(4)));
        }
        let n = usize::from(at(5));
        if n == 0 || n > MAX_WAVES {
            return Err(format!("a wavetable has 1 to {MAX_WAVES} waves, not {n}"));
        }
        let len = usize::from(u16::from_le_bytes([at(6), at(7)]));
        if len != WAVE_LEN {
            return Err(format!("waves of {len} samples (this build reads {WAVE_LEN})"));
        }
        let name_len = usize::from(at(8));
        if name_len > MAX_NAME_BYTES {
            return Err("the wavetable's name is too long".into());
        }
        let name = bytes.get(HEADER..HEADER + name_len).ok_or("the wavetable is cut short")?;
        let name = std::str::from_utf8(name).map_err(|_| "the wavetable's name is not text".to_string())?.to_string();
        let body = bytes.get(HEADER + name_len..).unwrap_or(&[]);
        if body.len() != n * WAVE_LEN * 4 {
            return Err(format!("the wavetable holds {} bytes of waves where {n} waves take {}", body.len(), n * WAVE_LEN * 4));
        }
        let mut waves = Vec::with_capacity(n);
        for chunk in body.chunks_exact(WAVE_LEN * 4) {
            let w: Vec<f32> = chunk.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap_or([0; 4]))).collect();
            if w.iter().any(|v| !v.is_finite()) {
                return Err("the wavetable holds non-finite samples".into());
            }
            waves.push(w);
        }
        Ok(UserTable { name, waves })
    }

    /// One cycle at `pos` (0..1 through the waves, crossfaded between two as the synth reads
    /// them) drawn into `out` (resampled to its length), scaled as the synth plays it (the whole
    /// table's loudest point at 1). For the panel.
    pub fn draw(&self, pos: f32, out: &mut [f32]) {
        let n = self.waves.len();
        let peak = self.waves.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()));
        let scale = if peak > 1e-9 { 1.0 / peak } else { 0.0 };
        let fp = if pos.is_finite() { pos.clamp(0.0, 1.0) } else { 0.0 } * n.saturating_sub(1) as f32;
        let i = (fp as usize).min(n.saturating_sub(1));
        let j = (i + 1).min(n.saturating_sub(1));
        let f = fp - i as f32;
        let len = out.len().max(1);
        for (k, o) in out.iter_mut().enumerate() {
            let x = k as f32 / len as f32 * WAVE_LEN as f32;
            let read = |w: usize| {
                let Some(w) = self.waves.get(w) else { return 0.0 };
                let a = (x as usize).min(WAVE_LEN - 1);
                let b = (a + 1) % WAVE_LEN;
                let fr = x - a as f32;
                let (va, vb) = (w.get(a).copied().unwrap_or(0.0), w.get(b).copied().unwrap_or(0.0));
                va + (vb - va) * fr
            };
            let a = read(i);
            *o = (a + (read(j) - a) * f) * scale;
        }
    }
}

fn cut_name(name: &str) -> String {
    let mut end = name.len().min(MAX_NAME_BYTES);
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    name.get(..end).unwrap_or("").to_string()
}

/// One cycle `w` resampled to `n` samples, linearly and periodically (non-finite samples as 0).
pub fn resample_cycle(w: &[f32], n: usize) -> Vec<f32> {
    let m = w.len();
    if m == n {
        return w.iter().map(|v| if v.is_finite() { *v } else { 0.0 }).collect();
    }
    if m == 0 {
        return vec![0.0; n];
    }
    let at = |i: usize| w.get(i % m).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
    (0..n)
        .map(|k| {
            let x = k as f64 * m as f64 / n as f64;
            let i = x as usize;
            let f = (x - i as f64) as f32;
            at(i) + (at(i + 1) - at(i)) * f
        })
        .collect()
}

/// A user table made ready to play: every wave at every band-limit level, laid out as one bank
/// of the built-in tables is ([`tables::FRAME_LEN`] samples a wave), the whole table scaled so
/// its loudest point is 1. Carried to the synth as a [`SampleSource`] whose one channel is that
/// data (not audio: it only travels the asset path).
#[derive(Debug, Clone)]
pub struct BuiltTable {
    data: Vec<f32>,
    name: String,
}

impl BuiltTable {
    /// Band-limits `t`'s waves (allocates, takes a few milliseconds: never on the audio thread).
    pub fn new(t: &UserTable) -> BuiltTable {
        let n = t.waves.len().min(MAX_WAVES);
        let mut data = vec![0.0f32; n * FRAME_LEN];
        let mut spec: Vec<C> = vec![(0.0, 0.0); WAVE_LEN];
        let mut buf: Vec<C> = Vec::with_capacity(size(0));
        let mut peak = 0.0f32;
        for (w, wave) in t.waves.iter().take(n).enumerate() {
            for (s, v) in spec.iter_mut().zip(wave.iter().chain(std::iter::repeat(&0.0))) {
                *s = (if v.is_finite() { f64::from(*v) } else { 0.0 }, 0.0);
            }
            fft(&mut spec, false);
            let base = w * FRAME_LEN;
            for level in 0..LEVELS {
                let len = size(level);
                let h = harmonics(level).min(len / 2 - 1).min(WAVE_LEN / 2 - 1);
                buf.clear();
                buf.resize(len, (0.0, 0.0));
                for k in 1..=h {
                    let c = spec.get(k).map_or((0.0, 0.0), |x| (x.0 / WAVE_LEN as f64, x.1 / WAVE_LEN as f64));
                    if let Some(x) = buf.get_mut(k) {
                        *x = c;
                    }
                    if let Some(x) = buf.get_mut(len - k) {
                        *x = (c.0, -c.1);
                    }
                }
                fft(&mut buf, true);
                let at = base + offset(level);
                if let Some(dst) = data.get_mut(at..at + len + 1) {
                    for (d, s) in dst.iter_mut().zip(buf.iter()) {
                        *d = s.0 as f32;
                        if level == 0 {
                            peak = peak.max(d.abs());
                        }
                    }
                    if let (Some(first), Some(guard)) = (buf.first(), dst.last_mut()) {
                        *guard = first.0 as f32;
                    }
                }
            }
        }
        let scale = if peak > 1e-9 && peak.is_finite() { 1.0 / peak } else { 0.0 };
        for d in &mut data {
            *d = if d.is_finite() { *d * scale } else { 0.0 };
        }
        BuiltTable { data, name: t.name.clone() }
    }

    /// Waves in the table.
    pub fn waves(&self) -> usize {
        self.data.len() / FRAME_LEN
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The band-limited data (see [`read`]).
    pub fn data(&self) -> &[f32] {
        &self.data
    }
}

impl SampleSource for BuiltTable {
    fn sample_rate(&self) -> f32 {
        48_000.0
    }
    fn num_channels(&self) -> usize {
        1
    }
    fn channel(&self, ch: usize) -> Option<&[f32]> {
        (ch == 0).then_some(self.data.as_slice())
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// The stored bytes of a user table made ready for the synth (decoded and band-limited). Off the
/// audio thread only. Errors say what is wrong with the bytes.
pub fn prepare(state: &[u8]) -> Result<Arc<dyn SampleSource>, String> {
    let t = UserTable::decode(state)?;
    Ok(Arc::new(BuiltTable::new(&t)))
}

/// The wave of a built table's `data` ([`BuiltTable::data`]) at `pos` (0..1 through its waves,
/// crossfaded between two), band limit `level`, at `phase` (0..1); 0 for empty data. As
/// [`tables::Tables::read`] reads a bank.
#[inline]
pub fn read(data: &[f32], pos: f32, level: usize, phase: f32) -> f32 {
    let frames = data.len() / FRAME_LEN;
    if frames == 0 {
        return 0.0;
    }
    let level = level.min(LEVELS - 1);
    let cycle = |w: usize| {
        let start = w * FRAME_LEN + offset(level);
        data.get(start..start + size(level) + 1).unwrap_or(&[])
    };
    let fp = pos.clamp(0.0, 1.0) * (frames - 1) as f32;
    let i = (fp as usize).min(frames - 1);
    let j = (i + 1).min(frames - 1);
    let f = fp - i as f32;
    let a = lerp_cycle(cycle(i), phase);
    if f <= 0.0 || i == j {
        return a;
    }
    a + (lerp_cycle(cycle(j), phase) - a) * f
}

/// The synth's hold on its user table: the one it plays, the one it is fading from, and those it
/// let go of for the host to take. Nothing here allocates.
pub(crate) struct Slot {
    cur: Option<Arc<dyn SampleSource>>,
    /// The table faded from (`None`: the built-in bank) while `fade` > 0.
    prev: Option<Arc<dyn SampleSource>>,
    fading: bool,
    fade: u32,
    fade_len: u32,
    released: [Option<Arc<dyn SampleSource>>; RELEASED],
}

impl Default for Slot {
    fn default() -> Self {
        Slot { cur: None, prev: None, fading: false, fade: 0, fade_len: 1, released: [None, None, None, None] }
    }
}

impl Slot {
    /// Whether a user table plays or is fading in or out.
    #[inline]
    pub(crate) fn active(&self) -> bool {
        self.cur.is_some() || self.fading
    }

    fn release(&mut self, a: Option<Arc<dyn SampleSource>>) -> Option<Arc<dyn SampleSource>> {
        let a = a?;
        match self.released.iter_mut().find(|r| r.is_none()) {
            Some(r) => {
                *r = Some(a);
                None
            }
            None => Some(a),
        }
    }

    /// Takes `table` (`None`: back to the built-in bank), fading from what played over
    /// [`FADE_MS`] at `sr` when `fade` (notes sound), else at once. `false` (nothing changed)
    /// when the slot has no room to give back a table it would let go of: the host tries again
    /// after taking the released ones.
    pub(crate) fn set(&mut self, table: Option<Arc<dyn SampleSource>>, sr: f32, fade: bool) -> bool {
        // A fade still running ends here: what it faded from is let go of.
        if self.prev.is_some() && self.released.iter().all(Option::is_some) {
            return false;
        }
        let old = self.prev.take();
        if let Some(back) = self.release(old) {
            // Not reached (room was checked), but never dropped here.
            self.prev = Some(back);
            return false;
        }
        self.prev = std::mem::replace(&mut self.cur, table);
        self.fade_len = ((FADE_MS * 0.001 * sr) as u32).max(1);
        self.fade = self.fade_len;
        self.fading = true;
        if !fade {
            self.snap();
        }
        true
    }

    /// Ends any fade at once (a reset): what it faded from is let go of when there is room.
    pub(crate) fn snap(&mut self) {
        self.fade = 0;
        self.fading = false;
        let old = self.prev.take();
        self.prev = self.release(old);
    }

    /// One released table for the host to drop.
    pub(crate) fn take_released(&mut self) -> Option<Arc<dyn SampleSource>> {
        self.released.iter_mut().find_map(Option::take)
    }

    /// The data played now (`None`: the built-in bank).
    #[inline]
    pub(crate) fn current(&self) -> Option<&[f32]> {
        self.cur.as_ref().and_then(|a| a.channel(0))
    }

    /// While fading: the data faded from (`Some(None)`: the built-in bank) and how much of it is
    /// left in the mix (1 → 0).
    #[inline]
    pub(crate) fn fading_from(&self) -> Option<(Option<&[f32]>, f32)> {
        self.fading.then(|| (self.prev.as_ref().and_then(|a| a.channel(0)), self.fade as f32 / self.fade_len as f32))
    }

    /// Moves the fade on one sample; at its end what it faded from is let go of.
    #[inline]
    pub(crate) fn step(&mut self) {
        if !self.fading {
            // One let go of while the slot had no room to give it back: as soon as it has.
            if self.prev.is_some() {
                let old = self.prev.take();
                self.prev = self.release(old);
            }
            return;
        }
        self.fade = self.fade.saturating_sub(1);
        if self.fade == 0 {
            self.fading = false;
            let old = self.prev.take();
            self.prev = self.release(old);
        }
    }
}

/// The wave at (`pos`, `level`, `phase`) from a user table's data, or bank `bank` of the built-in
/// tables when `data` is `None`.
#[inline]
pub(crate) fn read_either(t: &tables::Tables, data: Option<&[f32]>, bank: usize, pos: f32, level: usize, phase: f32) -> f32 {
    match data {
        Some(d) => read(d, pos, level, phase),
        None => t.read(bank, pos, level, phase),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saw(n: usize, harmonics: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let x = i as f64 / n as f64;
                (1..=harmonics).map(|k| (std::f64::consts::TAU * k as f64 * x).sin() / k as f64).sum::<f64>() as f32
            })
            .collect()
    }

    #[test]
    fn a_table_survives_its_stored_form_and_damage_is_refused() {
        let t = UserTable::new("Grain ✓", &[saw(WAVE_LEN, 5), saw(512, 3)]).unwrap();
        assert_eq!(t.waves.len(), 2);
        assert!(t.waves.iter().all(|w| w.len() == WAVE_LEN));
        let bytes = t.encode();
        assert_eq!(UserTable::decode(&bytes).unwrap(), t);
        // Every way of damaging it is an error, never a panic.
        assert!(UserTable::decode(&[]).is_err());
        assert!(UserTable::decode(b"FDWT").is_err());
        assert!(UserTable::decode(&bytes[..bytes.len() - 1]).is_err());
        let mut more = bytes.clone();
        more.push(0);
        assert!(UserTable::decode(&more).is_err());
        for (at, v) in [(0, b'X'), (4, 9), (5, 0), (5, 33), (7, 0), (8, 200)] {
            let mut b = bytes.clone();
            b[at] = v;
            assert!(UserTable::decode(&b).is_err(), "byte {at} = {v}");
        }
        let mut nan = bytes.clone();
        let at = nan.len() - 4;
        nan[at..].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(UserTable::decode(&nan).is_err());
        let mut bad_name = bytes.clone();
        bad_name[HEADER] = 0xff;
        assert!(UserTable::decode(&bad_name).is_err());
        // Random bytes: refused or read, never a panic.
        let mut x = 0x1234_5678u32;
        for len in [0usize, 9, 40, 9 + 8192, 9 + 8192 * 3] {
            let junk: Vec<u8> = (0..len)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    x as u8
                })
                .collect();
            let _ = UserTable::decode(&junk);
            let _ = prepare(&junk);
        }
        assert!(UserTable::new("x", &[]).is_none());
        assert_eq!(UserTable::new(&"é".repeat(40), &[vec![0.0; 8]]).unwrap().name.len(), 64);
    }

    #[test]
    fn a_built_table_is_band_limited_as_the_built_in_ones_are() {
        // A wave with every harmonic up to 1000 keeps only 512 at the fullest level and 1 at the
        // last, each level normalised with the table (peak 1 at level 0) and with its guard.
        let t = UserTable::new("full", &[saw(WAVE_LEN, 1000), vec![0.0; WAVE_LEN]]).unwrap();
        let b = BuiltTable::new(&t);
        assert_eq!(b.waves(), 2);
        let top = &b.data()[offset(0)..offset(0) + size(0) + 1];
        let peak = top.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((peak - 1.0).abs() < 1e-4, "{peak}");
        assert_eq!(top.first(), top.last(), "guard");
        // The last level is a single sine: read through it at a quarter cycle it is at its peak.
        let last = LEVELS - 1;
        let q = read(b.data(), 0.0, last, 0.25);
        let s = read(b.data(), 0.0, last, 0.75);
        assert!(q > 0.1 && (q + s).abs() < 1e-3, "{q} {s}");
        // Position 1 is the silent second wave; halfway is half of the first.
        assert!(read(b.data(), 1.0, 0, 0.1).abs() < 1e-6);
        let half = read(b.data(), 0.5, 3, 0.1);
        let full = read(b.data(), 0.0, 3, 0.1);
        assert!((half - full * 0.5).abs() < 1e-5);
        // Empty or short data reads silence.
        assert_eq!(read(&[], 0.3, 0, 0.3), 0.0);
        assert_eq!(read(&[1.0; 10], 0.3, 0, 0.3), 0.0);
        // A silent table builds and is silent.
        let quiet = BuiltTable::new(&UserTable::new("q", &[vec![0.0; WAVE_LEN]]).unwrap());
        assert!(quiet.data().iter().all(|v| *v == 0.0));
    }

    #[test]
    fn the_slot_fades_swaps_and_gives_back_without_dropping() {
        let a: Arc<dyn SampleSource> = Arc::new(BuiltTable::new(&UserTable::new("a", &[saw(WAVE_LEN, 3)]).unwrap()));
        let b: Arc<dyn SampleSource> = Arc::new(BuiltTable::new(&UserTable::new("b", &[saw(WAVE_LEN, 9)]).unwrap()));
        let mut s = Slot::default();
        assert!(!s.active() && s.current().is_none());
        assert!(s.set(Some(Arc::clone(&a)), 1000.0, true));
        assert!(s.active() && s.current().is_some());
        let (from, left) = s.fading_from().unwrap();
        assert!(from.is_none() && left == 1.0, "fading in from the built-in bank");
        for _ in 0..20 {
            s.step();
        }
        assert!(s.fading_from().is_none());
        // Swaps in quick succession: the slot never drops a table, it hands each back.
        for _ in 0..2 {
            assert!(s.set(Some(Arc::clone(&b)), 1000.0, true));
            assert!(s.set(Some(Arc::clone(&a)), 1000.0, true));
        }
        let mut back = 0;
        while let Some(x) = s.take_released() {
            assert!(Arc::strong_count(&x) >= 2, "the host's own reference is still held");
            back += 1;
        }
        assert_eq!(back, 3);
        // Full of released tables, it refuses until they are taken.
        let mut refused = false;
        for _ in 0..10 {
            if !s.set(Some(Arc::clone(&b)), 1000.0, true) {
                refused = true;
                break;
            }
        }
        assert!(refused);
        while s.take_released().is_some() {}
        assert!(s.set(None, 1000.0, true));
        s.snap();
        assert!(!s.active());
    }
}
