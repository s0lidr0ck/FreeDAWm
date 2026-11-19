//! FreeDAWm's ARA 2 host (Audio Random Access, by Celemony): a plug-in such as an ARA pitch
//! editor reads a clip's audio straight from the host and follows its edits, instead of
//! capturing it as it plays.
//!
//! - [`sys`]: hand-written `#[repr(C)]` bindings to the ARA C API (`ARAInterface.h`, and the VST3
//!   companion ids of `ARAVST3.h`, ARA SDK 2.3, Apache-2.0; the headers are vendored unmodified in
//!   `vendor/ARA_API/`), with compile-time size and offset checks for every struct.
//! - [`Document`]: one ARA document controller of one plug-in and the host interfaces it calls
//!   back (audio access, archiving, content access, model update, playback), kept in line with a
//!   [`Graph`] of region sequences, audio sources and regions by [`Document::sync`] in editing
//!   cycles. Regions that go are deactivated for the undo history and come back on undo.
//! - `test_plugin` (feature `test-plugin`): the smallest ARA plug-in side, in Rust, against the
//!   same bindings, which reads every audio source it is given through the host and reports what
//!   it read as notes.
//!
//! - [`BoundInstance`]: a plug-in instance bound to a document controller (playback renderer,
//!   editor renderer, editor view roles) kept together with the extension its binding gave, so
//!   the extension is never used once the instance is gone; [`InstanceCell`] shares the instance
//!   with the audio thread, which never waits for it.
//! - Archiving: [`Document::store`] and [`Document::restore_next_sync`] (ARA 2's
//!   `storeObjectsToArchive` / `restoreObjectsFromArchive`, the deprecated ARA 1 calls for a
//!   plug-in without them). Playback requests reach the host as [`PlaybackRequest`]s.
//!
//! What the user sees is in `docs/ara.md`.
//!
//! # Threads
//!
//! - The **model thread**: every call into a document controller (everything [`Document`] does)
//!   is made on one thread, as ARA requires. `Document` is neither `Send` nor `Sync`, so it stays
//!   on the thread that made it; FreeDAWm uses the UI thread. Factories are initialised and
//!   uninitialised there too.
//! - **Reader threads**: the plug-in may call `createAudioReaderForSource`, `readAudioSamples`
//!   and `destroyAudioReader` from any non-realtime thread, several readers at once (one thread
//!   per reader at a time). These callbacks only touch locked tables shared with the document and
//!   read through [`AudioSource::read`], which must be safe from any thread; they never touch the
//!   document itself and never run on the audio thread.
//! - **The audio thread** only calls [`InstanceCell::try_with`] (a `try_lock`: it never waits) to
//!   render with a bound instance; nothing in a document.
//!
//! # A misbehaving plug-in
//!
//! Versioned structs are copied only as far as their `structSize` reaches and refused below the
//! header's minimum sizes; null function pointers are skipped (or the document is refused when an
//! essential one is missing); generations outside what this host implements are refused; strings
//! and lists are read with caps; every host callback catches panics, checks every ref it is given
//! against its tables and every count against limits, and answers a bad call with `false`, null or
//! silence. What no host can catch: a plug-in that crashes or corrupts memory in its own code.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod log;
pub mod sys;

#[allow(unsafe_code, non_upper_case_globals)]
mod host;

#[cfg(any(test, feature = "test-plugin"))]
#[allow(unsafe_code, non_upper_case_globals)]
pub mod test_plugin;

#[cfg(test)]
#[allow(unsafe_code, non_upper_case_globals)]
mod tests;

pub use host::{BoundInstance, Document, FactoryPtr, HOST_GENERATIONS, InstanceCell, RawExtension, choose_generation, read_factory};
pub use log::{log_file, recent as recent_log, set_log_file};

use std::sync::Arc;

/// Audio an ARA audio source reads: a file of the session. Read from the plug-in's reader threads,
/// so it must be safe to read from any thread (never the audio thread).
pub trait AudioSource: Send + Sync {
    fn channels(&self) -> usize;
    fn frames(&self) -> u64;
    fn sample_rate(&self) -> f64;
    /// Fills `out` with channel `ch` from frame `start` (frames past the end are zero). `false`
    /// when the audio could not be read (the host then gives the plug-in silence and says so).
    fn read(&self, ch: usize, start: u64, out: &mut [f32]) -> bool;
    /// Whether the audio holds more than 32-bit float samples can (FreeDAWm's never does).
    fn merits_64_bit(&self) -> bool {
        false
    }
}

/// A track: an ARA region sequence.
#[derive(Debug, Clone, PartialEq)]
pub struct SequenceDesc {
    pub key: u64,
    pub name: String,
    /// The track's place, top to bottom.
    pub order: i32,
    pub color: Option<[f32; 3]>,
}

/// An audio file: an ARA audio source.
#[derive(Clone)]
pub struct SourceDesc {
    pub key: u64,
    pub name: String,
    /// Unique in the document and stable across sessions (ARA archives refer to it).
    pub persistent_id: String,
    pub audio: Arc<dyn AudioSource>,
}

impl std::fmt::Debug for SourceDesc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceDesc").field("key", &self.key).field("name", &self.name).field("persistent_id", &self.persistent_id).finish()
    }
}

/// A clip: an ARA audio modification of its source with one playback region on its track's
/// sequence. Times in seconds. A time-stretched clip plays `source_duration` of its source over
/// `duration` (only offered with [`sys::kARAPlaybackTransformationTimestretch`] in
/// `transformation`, for a plug-in that supports it).
#[derive(Debug, Clone, PartialEq)]
pub struct RegionDesc {
    pub key: u64,
    pub source: u64,
    pub sequence: u64,
    pub name: String,
    /// Unique in the document and stable across sessions.
    pub persistent_id: String,
    /// Where in the source the clip starts.
    pub start_in_source: f64,
    /// How long it plays on the timeline.
    pub duration: f64,
    /// How much of the source it plays (equal to `duration` unless time-stretched).
    pub source_duration: f64,
    /// `sys::kARAPlaybackTransformation*` flags of its playback region.
    pub transformation: i32,
    /// The region whose audio modification this one starts as a copy of (a duplicated or split
    /// clip: `cloneAudioModification`, when the plug-in has it and that modification is live).
    pub origin: Option<u64>,
    /// Where on the timeline it starts.
    pub start_in_song: f64,
    pub color: Option<[f32; 3]>,
}

/// What an ARA document should hold.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    /// The document's name (the session's).
    pub name: String,
    pub sequences: Vec<SequenceDesc>,
    pub sources: Vec<SourceDesc>,
    pub regions: Vec<RegionDesc>,
    /// The tempo map as ARA wants it: `(seconds, quarter notes)` sync points, at least two, one at
    /// quarter 0, ascending. Anything else is not offered to the plug-in.
    pub tempo: Vec<(f64, f64)>,
    /// Bar signatures `(numerator, denominator, quarter position)`, ascending (only with a tempo).
    pub bars: Vec<(i32, i32, f64)>,
}

/// Most of each kind of object one document takes.
const MAX_OBJECTS: usize = 1 << 16;
/// Most audio channels of one source.
const MAX_CHANNELS: usize = 64;

impl Graph {
    /// The graph with what ARA cannot take left out (logged): duplicate keys, sources without
    /// channels or rate, regions naming a missing source or sequence or with times that are not
    /// finite or not positive, an invalid tempo map.
    pub fn valid(&self) -> Graph {
        let mut out = Graph { name: self.name.clone(), ..Graph::default() };
        let mut seen = std::collections::BTreeSet::new();
        for s in self.sequences.iter().take(MAX_OBJECTS) {
            if seen.insert(s.key) {
                out.sequences.push(s.clone());
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for s in self.sources.iter().take(MAX_OBJECTS) {
            let rate = s.audio.sample_rate();
            let ok = (1..=MAX_CHANNELS).contains(&s.audio.channels()) && rate.is_finite() && rate > 0.0 && i64::try_from(s.audio.frames()).is_ok();
            if !ok {
                crate::ara_log!("audio source \"{}\" left out: {} channels at {rate} Hz", s.name, s.audio.channels());
            } else if seen.insert(s.key) {
                out.sources.push(s.clone());
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for r in self.regions.iter().take(MAX_OBJECTS) {
            let times = [r.start_in_source, r.duration, r.start_in_song, r.source_duration];
            let ok = times.iter().all(|t| t.is_finite())
                && r.duration > 0.0
                && r.source_duration > 0.0
                && r.start_in_source >= 0.0
                && out.sources.iter().any(|s| s.key == r.source)
                && out.sequences.iter().any(|s| s.key == r.sequence);
            if !ok {
                crate::ara_log!("region \"{}\" left out (times {times:?}, or its source or track is missing)", r.name);
            } else if seen.insert(r.key) {
                out.regions.push(r.clone());
            }
        }
        let tempo_ok = self.tempo.len() >= 2
            && self.tempo.iter().all(|(t, q)| t.is_finite() && q.is_finite())
            && self.tempo.windows(2).all(|w| matches!(w, [a, b] if b.0 > a.0 && b.1 > a.1))
            && self.tempo.iter().any(|(_, q)| *q == 0.0);
        if tempo_ok {
            out.tempo = self.tempo.iter().take(MAX_OBJECTS).copied().collect();
            let bars_ok =
                self.bars.iter().all(|(n, d, q)| *n > 0 && *d > 0 && q.is_finite()) && self.bars.windows(2).all(|w| matches!(w, [a, b] if b.2 > a.2));
            if bars_ok {
                out.bars = self.bars.iter().take(MAX_OBJECTS).copied().collect();
            }
        }
        out
    }
}

/// What a plug-in's ARA factory says about itself (copied, capped).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FactoryInfo {
    pub name: String,
    pub manufacturer: String,
    pub url: String,
    pub version: String,
    /// Unique, versioned plug-in id (changes when its capabilities change).
    pub factory_id: String,
    pub document_archive_id: String,
    pub compatible_archive_ids: Vec<String>,
    pub lowest_generation: i32,
    pub highest_generation: i32,
    /// The generation this host would use (`None`: it cannot host this plug-in).
    pub generation: Option<i32>,
    /// What it can analyse (`sys::kARAContentType*`).
    pub content_types: Vec<i32>,
    /// The playback transformations it supports (`sys::kARAPlaybackTransformation*`).
    pub transformations: i32,
    pub audio_file_chunks: bool,
}

impl FactoryInfo {
    /// The content types as words ("notes", "tempo", …).
    pub fn content_names(&self) -> Vec<&'static str> {
        self.content_types
            .iter()
            .map(|t| match *t {
                sys::kARAContentTypeNotes => "notes",
                sys::kARAContentTypeTempoEntries => "tempo",
                sys::kARAContentTypeBarSignatures => "bar signatures",
                sys::kARAContentTypeStaticTuning => "tuning",
                sys::kARAContentTypeKeySignatures => "key signatures",
                sys::kARAContentTypeSheetChords => "chords",
                _ => "other",
            })
            .collect()
    }
}

/// Why ARA could not be used with a plug-in.
#[derive(Debug, Clone, PartialEq)]
pub enum AraError {
    /// ARA needs a 64-bit build.
    Unsupported,
    /// The factory is unusable.
    Factory(String),
    /// No API generation both sides support (the plug-in's lowest and highest).
    Generation(i32, i32),
    /// The document controller it made is unusable.
    Controller(String),
}

impl std::fmt::Display for AraError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AraError::Unsupported => write!(f, "ARA needs a 64-bit build"),
            AraError::Factory(m) => write!(f, "unusable ARA factory: {m}"),
            AraError::Generation(lo, hi) => {
                write!(f, "the plug-in supports ARA API generations {lo} to {hi}; FreeDAWm needs ARA 2 (generation 4 or 6)")
            }
            AraError::Controller(m) => write!(f, "unusable ARA document controller: {m}"),
        }
    }
}

impl std::error::Error for AraError {}

/// Analysis progress a plug-in reported for an audio source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisState {
    Started,
    Running,
    Completed,
}

/// A notification from the plug-in, by graph key.
#[derive(Debug, Clone, PartialEq)]
pub enum HostEvent {
    Analysis {
        source: u64,
        state: AnalysisState,
        progress: f32,
    },
    /// What the plug-in knows of a source changed (e.g. its analysis finished).
    SourceContentChanged(u64),
    /// A region's (or its modification's) content changed: the user edited it in the plug-in.
    RegionContentChanged(u64),
    /// The plug-in's own document data changed (it wants saving: stage two).
    DocumentDataChanged,
    /// The plug-in asked the host to do something with playback (ARA's playback controller).
    PlaybackRequest(PlaybackRequest),
}

/// What a plug-in asked of the host's transport (ARA's playback controller interface).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlaybackRequest {
    Start,
    Stop,
    /// Locate to a song position (seconds).
    Position(f64),
    /// Set the loop range (start, duration in seconds).
    Cycle(f64, f64),
    /// Turn looping on or off.
    EnableCycle(bool),
}

impl PlaybackRequest {
    /// In words, for the log.
    pub fn describe(&self) -> String {
        match self {
            PlaybackRequest::Start => "start playback".into(),
            PlaybackRequest::Stop => "stop playback".into(),
            PlaybackRequest::Position(t) => format!("set the playback position to {t:.3} s"),
            PlaybackRequest::Cycle(s, d) => format!("set the loop to {s:.3} s for {d:.3} s"),
            PlaybackRequest::EnableCycle(on) => format!("turn looping {}", if *on { "on" } else { "off" }),
        }
    }
}

/// A note the plug-in found (ARA's `ARAContentNote`: seconds, Hz, MIDI pitch).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Note {
    pub frequency: f32,
    pub pitch: i32,
    pub volume: f32,
    pub start: f64,
    pub attack: f64,
    pub duration: f64,
    pub signal: f64,
}

/// What a document's plug-in read through the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReadStats {
    /// `readAudioSamples` calls answered.
    pub reads: u64,
    /// Samples given (all channels).
    pub samples: u64,
    /// Calls refused or answered with silence.
    pub failed: u64,
    pub open_readers: usize,
}

/// What one [`Document::sync`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyncReport {
    pub created: usize,
    pub updated: usize,
    pub removed: usize,
    pub reactivated: usize,
    pub deactivated: usize,
}

impl SyncReport {
    /// Whether anything changed.
    pub fn changed(&self) -> bool {
        *self != SyncReport::default()
    }
}

/// An [`AudioSource`] held in memory (tests and small files).
#[derive(Debug, Clone)]
pub struct MemorySource {
    pub channels: Vec<Vec<f32>>,
    pub rate: f64,
}

impl AudioSource for MemorySource {
    fn channels(&self) -> usize {
        self.channels.len()
    }
    fn frames(&self) -> u64 {
        self.channels.first().map_or(0, |c| c.len() as u64)
    }
    fn sample_rate(&self) -> f64 {
        self.rate
    }
    fn read(&self, ch: usize, start: u64, out: &mut [f32]) -> bool {
        out.iter_mut().for_each(|x| *x = 0.0);
        let Some(c) = self.channels.get(ch) else { return false };
        let from = usize::try_from(start).unwrap_or(usize::MAX).min(c.len());
        for (o, v) in out.iter_mut().zip(c.get(from..).unwrap_or_default()) {
            *o = *v;
        }
        true
    }
}
