//! Audio sources (files in the session's Audio Files folder) and their decoded data, and the
//! movies video clips reference.

use crate::SourceId;
use soundcraft_audio_io::{AudioBuffer, FileFormat, peaks::Peaks};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Metadata for an audio file used by the session (serialised).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Source {
    pub id: SourceId,
    pub name: String,
    /// Path relative to the session folder (or absolute for referenced media).
    pub path: String,
    pub channels: u16,
    pub frames: u64,
    pub sample_rate: u32,
    pub format: FileFormat,
    /// Original BWF time stamp (samples), if any.
    #[serde(default)]
    pub time_reference: u64,
    /// True when the file has not been written to disk yet (recorded/rendered this session).
    #[serde(default)]
    pub unsaved: bool,
}

/// A movie used by Video track clips (serialised). The pictures themselves are never stored in
/// the session: the UI opens the movie from `path` and decodes frames on demand.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VideoSource {
    pub id: SourceId,
    pub name: String,
    /// Absolute path of the movie (referenced, never copied into the session folder).
    pub path: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    /// Frames per second (0 = unknown).
    #[serde(default)]
    pub frame_rate: f64,
    /// Picture length in seconds.
    #[serde(default)]
    pub duration: f64,
    /// Codec id (`h264`, `prores`, `mjpeg`, `hevc`, …).
    #[serde(default)]
    pub codec: String,
    /// Why the movie is not opened, when its stored path was refused on opening the session.
    #[serde(skip)]
    pub offline: Option<String>,
}

impl VideoSource {
    /// Display aspect ratio (width / height), 16:9 when unknown.
    pub fn aspect(&self) -> f64 {
        if self.width == 0 || self.height == 0 { 16.0 / 9.0 } else { f64::from(self.width) / f64::from(self.height) }
    }
}

/// A source's audio plus its waveform overviews (not serialised): decoded in memory, or a WAV
/// played from disk through the block cache ([`crate::disk`]). Read it through [`Self::read`]
/// (blocking), [`Self::cursor`] (the mix engine) or [`Self::buffer`] (the whole file).
#[derive(Debug)]
pub struct SourceAudio {
    data: SourceData,
    pub peaks: Vec<Peaks>,
}

#[derive(Debug)]
enum SourceData {
    Memory(AudioBuffer),
    Disk(Arc<crate::disk::DiskFile>),
}

impl SourceAudio {
    pub fn new(buffer: AudioBuffer) -> Self {
        let peaks = buffer.channels.iter().map(|c| Peaks::build(c)).collect();
        SourceAudio { data: SourceData::Memory(buffer), peaks }
    }
    /// With overviews made before (the waveform cache): one per channel, each of that channel's
    /// length, else the buffer back.
    pub fn with_peaks(buffer: AudioBuffer, peaks: Vec<Peaks>) -> Result<Self, AudioBuffer> {
        if peaks.len() != buffer.channels.len() || peaks.iter().zip(&buffer.channels).any(|(p, c)| p.len_samples() != c.len()) {
            return Err(buffer);
        }
        Ok(SourceAudio { data: SourceData::Memory(buffer), peaks })
    }
    /// Played from disk, with its overviews (one per channel, each the file's length), else the
    /// file back.
    pub fn from_disk(file: Arc<crate::disk::DiskFile>, peaks: Vec<Peaks>) -> Result<Self, Arc<crate::disk::DiskFile>> {
        if peaks.len() != file.channels() || peaks.iter().any(|p| p.len_samples() != file.frames()) {
            return Err(file);
        }
        Ok(SourceAudio { data: SourceData::Disk(file), peaks })
    }
    /// Played from disk, its overviews built by reading the file through once (blocking).
    pub fn from_disk_scanned(file: Arc<crate::disk::DiskFile>) -> Self {
        let mut builders: Vec<soundcraft_audio_io::peaks::PeaksBuilder> = (0..file.channels()).map(|_| Default::default()).collect();
        file.scan(|planar, n| {
            for (b, ch) in builders.iter_mut().zip(planar) {
                b.push(ch.get(..n).unwrap_or_default());
            }
        });
        SourceAudio { data: SourceData::Disk(file), peaks: builders.into_iter().map(|b| b.finish()).collect() }
    }
    pub fn frames(&self) -> usize {
        match &self.data {
            SourceData::Memory(b) => b.frames(),
            SourceData::Disk(f) => f.frames(),
        }
    }
    pub fn num_channels(&self) -> usize {
        match &self.data {
            SourceData::Memory(b) => b.num_channels(),
            SourceData::Disk(f) => f.channels(),
        }
    }
    pub fn sample_rate(&self) -> u32 {
        match &self.data {
            SourceData::Memory(b) => b.sample_rate,
            SourceData::Disk(f) => f.sample_rate(),
        }
    }
    /// The decoded audio when it is held in memory (`None` when played from disk).
    pub fn in_memory(&self) -> Option<&AudioBuffer> {
        match &self.data {
            SourceData::Memory(b) => Some(b),
            SourceData::Disk(_) => None,
        }
    }
    /// The file it plays from, when played from disk.
    pub fn disk_file(&self) -> Option<&Arc<crate::disk::DiskFile>> {
        match &self.data {
            SourceData::Memory(_) => None,
            SourceData::Disk(f) => Some(f),
        }
    }
    /// One channel held in memory (`None` when played from disk or out of range).
    pub fn channel(&self, ch: usize) -> Option<&[f32]> {
        self.in_memory()?.channels.get(ch).map(Vec::as_slice)
    }
    /// The whole audio (blocking: a file played from disk is read now; memory is borrowed).
    pub fn buffer(&self) -> std::borrow::Cow<'_, AudioBuffer> {
        match &self.data {
            SourceData::Memory(b) => std::borrow::Cow::Borrowed(b),
            SourceData::Disk(f) => std::borrow::Cow::Owned(AudioBuffer { sample_rate: f.sample_rate(), channels: f.read_all() }),
        }
    }
    /// Reads channel `ch` from frame `start` into `out` (blocking; not on the audio thread).
    /// Frames outside the audio read as silence; returns how many were inside it.
    pub fn read(&self, ch: usize, start: usize, out: &mut [f32]) -> usize {
        match &self.data {
            SourceData::Memory(b) => {
                out.iter_mut().for_each(|x| *x = 0.0);
                let Some(src) = b.channels.get(ch) else { return 0 };
                let Some(src) = src.get(start.min(src.len())..) else { return 0 };
                let n = src.len().min(out.len());
                for (d, v) in out.iter_mut().zip(src.iter().take(n)) {
                    *d = *v;
                }
                n
            }
            SourceData::Disk(f) => f.read(ch, start, out),
        }
    }
    /// Every channel over `[start, start + n)` (blocking); positions outside read as silence.
    pub fn window(&self, start: i64, n: usize) -> Vec<Vec<f32>> {
        (0..self.num_channels())
            .map(|ch| {
                let mut v = vec![0.0; n];
                let skip = usize::try_from(start.saturating_neg()).unwrap_or(0).min(n);
                if let (Ok(from), Some(dst)) = (usize::try_from(start.max(0)), v.get_mut(skip..)) {
                    self.read(ch, from, dst);
                }
                v
            })
            .collect()
    }
    /// A position-by-position reader of channel `ch` (out of range: an empty one). `realtime`:
    /// never blocks (the audio thread; a block not loaded yet reads as silence); else what isn't
    /// cached is read when reached.
    pub fn cursor(&self, ch: usize, realtime: bool) -> SampleCursor<'_> {
        match &self.data {
            SourceData::Memory(b) => SampleCursor::Memory(b.channels.get(ch).map(Vec::as_slice).unwrap_or(&[])),
            SourceData::Disk(f) if ch < f.channels() => SampleCursor::Disk(crate::disk::Cursor::new(f, ch, !realtime)),
            SourceData::Disk(_) => SampleCursor::Memory(&[]),
        }
    }
}

/// Reads one channel of a [`SourceAudio`] position by position ([`SourceAudio::cursor`]).
pub enum SampleCursor<'a> {
    Memory(&'a [f32]),
    Disk(crate::disk::Cursor<'a>),
}

impl SampleCursor<'_> {
    /// Sample `x`, `None` past the end.
    #[inline]
    pub fn get(&mut self, x: usize) -> Option<f32> {
        match self {
            SampleCursor::Memory(s) => s.get(x).copied(),
            SampleCursor::Disk(c) => c.get(x),
        }
    }
    /// Copies positions `[start, start + out.len())` into `out`, silence outside the audio
    /// (negative positions too): what [`SampleCursor::get`] reads, a run at a time.
    #[inline]
    pub fn fill(&mut self, start: i64, out: &mut [f32]) {
        let skip = usize::try_from(start.saturating_neg()).unwrap_or(0).min(out.len());
        let (head, tail) = out.split_at_mut(skip);
        head.fill(0.0);
        let from = usize::try_from(start.max(0)).unwrap_or(usize::MAX);
        match self {
            SampleCursor::Memory(s) => {
                let src = s.get(from.min(s.len())..).unwrap_or_default();
                let n = src.len().min(tail.len());
                let (a, b) = tail.split_at_mut(n);
                for (d, v) in a.iter_mut().zip(src) {
                    *d = *v;
                }
                b.fill(0.0);
            }
            SampleCursor::Disk(c) => c.fill(from, tail),
        }
    }
    /// Whether it reads nothing (a missing channel).
    pub fn is_empty(&self) -> bool {
        matches!(self, SampleCursor::Memory(s) if s.is_empty())
    }
}

/// Shared decoded audio, keyed by source id.
#[derive(Debug, Clone, Default)]
pub struct SourcePool {
    map: BTreeMap<SourceId, Arc<SourceAudio>>,
}

impl SourcePool {
    pub fn insert(&mut self, id: SourceId, audio: Arc<SourceAudio>) {
        self.map.insert(id, audio);
    }
    pub fn get(&self, id: SourceId) -> Option<&Arc<SourceAudio>> {
        self.map.get(&id)
    }
    pub fn remove(&mut self, id: SourceId) -> Option<Arc<SourceAudio>> {
        self.map.remove(&id)
    }
    pub fn contains(&self, id: SourceId) -> bool {
        self.map.contains_key(&id)
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    pub fn ids(&self) -> impl Iterator<Item = SourceId> + '_ {
        self.map.keys().copied()
    }
}

impl PartialEq for SourcePool {
    fn eq(&self, other: &Self) -> bool {
        self.map.len() == other.map.len() && self.map.iter().zip(other.map.iter()).all(|((a, x), (b, y))| a == b && Arc::ptr_eq(x, y))
    }
}

/// A module asset made ready for its plugin (FireStation Amp's cabinet impulse response, decoded,
/// resampled and partitioned), opaque to the document: the mix engine hands it to the plugin.
/// The value inside is an `Arc<dyn soundcraft_dsp::SampleSource>`.
#[derive(Clone)]
pub struct AssetData(pub Arc<dyn std::any::Any + Send + Sync>);

impl std::fmt::Debug for AssetData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AssetData")
    }
}

impl PartialEq for AssetData {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Module assets made ready, keyed by their reference (an `Insert::assets` value: `bundled:<name>`
/// or a file path) and the sample rate they were made for (not serialised; the engine makes them
/// when a module's asset is set and when a session opens). Two kinds are kept apart, as the same
/// file can be both: prepared assets (an Amp's cabinet IR) and reference audio (the slots
/// [`REFERENCE_KEYS`](crate::REFERENCE_KEYS) name: the whole file at the session rate).
/// [`AssetPool::get_for`] picks the kind from the module's asset key.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AssetPool {
    /// reference -> rate -> data: nested so a lookup borrows the `&str` (the mixer's per-block
    /// structure check reads it on the audio thread, which must not allocate).
    map: BTreeMap<String, BTreeMap<u32, AssetData>>,
    /// The same for reference audio.
    audio: BTreeMap<String, BTreeMap<u32, AssetData>>,
}

impl AssetPool {
    /// A prepared asset (not reference audio).
    pub fn insert(&mut self, reference: &str, rate: u32, data: AssetData) {
        self.map.entry(reference.to_string()).or_default().insert(rate, data);
    }
    /// The prepared asset `reference` names at `rate`. Does not allocate.
    pub fn get(&self, reference: &str, rate: u32) -> Option<&AssetData> {
        self.map.get(reference)?.get(&rate)
    }
    pub fn contains(&self, reference: &str, rate: u32) -> bool {
        self.get(reference, rate).is_some()
    }
    /// Stores the asset made for module asset key `key` (its kind follows the key).
    pub fn insert_for(&mut self, key: &str, reference: &str, rate: u32, data: AssetData) {
        let map = if crate::is_reference_key(key) { &mut self.audio } else { &mut self.map };
        map.entry(reference.to_string()).or_default().insert(rate, data);
    }
    /// The asset module asset key `key` takes from `reference` at `rate`. Does not allocate.
    pub fn get_for(&self, key: &str, reference: &str, rate: u32) -> Option<&AssetData> {
        let map = if crate::is_reference_key(key) { &self.audio } else { &self.map };
        map.get(reference)?.get(&rate)
    }
    pub fn contains_for(&self, key: &str, reference: &str, rate: u32) -> bool {
        self.get_for(key, reference, rate).is_some()
    }
    /// The number of (kind, reference, rate) entries.
    pub fn len(&self) -> usize {
        self.map.values().chain(self.audio.values()).map(BTreeMap::len).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.map.values().chain(self.audio.values()).all(BTreeMap::is_empty)
    }
}
