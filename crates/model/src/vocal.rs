//! Vocal tuning on audio clips: the Vocal track flag's name match, the per-clip tuning
//! ([`VocalTune`], saved) and the derived cache of analyses and tuned renders ([`VocalCache`],
//! never saved; recomputed in the background after load).

use crate::{Clip, ClipId, Session, SourceId};
use soundcraft_time::Samples;
use soundcraft_vocal::{EditCurves, NoteEdit, NoteId, NoteMode, SegOp, VocalAnalysis};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Whole words that mark a vocal track (case-insensitive, trailing digits ignored).
const VOCAL_WORDS: [&str; 15] =
    ["vox", "vocal", "vocals", "voc", "vo", "bgv", "bgvs", "bv", "bvs", "lead", "harmony", "harm", "choir", "singer", "voice"];
/// Words that make a lone `lead` an instrument.
const INSTRUMENT_WORDS: [&str; 4] = ["guitar", "gtr", "synth", "keys"];

/// Whether a track name reads as a vocal (the Vocal flag's default when the owner hasn't set
/// it). Words are the name split on anything but letters and digits, lowercased, trailing digits
/// dropped (`Vox2` is `vox`; `Voxengo` stays `voxengo` and doesn't match), so `vox`/`vocal` count
/// as prefixes only before the end of the word or a digit. `lead` alone doesn't count when the
/// name also names an instrument (`Lead Guitar`).
pub fn looks_vocal(name: &str) -> bool {
    let words: Vec<String> = name
        .split(|c: char| !c.is_alphanumeric())
        .map(|w| w.to_lowercase().trim_end_matches(|c: char| c.is_ascii_digit()).to_string())
        .filter(|w| !w.is_empty())
        .collect();
    let count = |list: &[&str]| words.iter().filter(|w| list.contains(&w.as_str())).count();
    let vocal = count(&VOCAL_WORDS);
    if vocal == 0 {
        return false;
    }
    let lone_lead = vocal == 1 && words.iter().any(|w| w == "lead");
    !(lone_lead && count(&INSTRUMENT_WORDS) > 0)
}

/// An audio clip's vocal tuning (saved with the clip): the note edits, the segmentation changes
/// replayed after every analysis, the Tuned/Original switch and the analysis the edits refer to.
/// Equality ignores [`VocalTune::fingerprint`] (derived).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct VocalTune {
    #[serde(default)]
    pub edits: BTreeMap<NoteId, NoteEdit>,
    #[serde(default)]
    pub seg_ops: Vec<SegOp>,
    /// The A/B switch: true plays the untuned clip.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub original: bool,
    /// [`AnalysisKey::hash`] of the analysis the edits refer to (0 before the first analysis).
    #[serde(default)]
    pub analysed: u64,
    /// The `(start, end-exclusive)` analysis frames of every note with an edit, so saved edits
    /// can be matched again by overlap when a reload analyses slightly differently.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub note_spans: BTreeMap<NoteId, (u32, u32)>,
    /// The source the tuning was made against (set when it is created or analysed). Once the
    /// clip plays another source (Consolidate, Process) the tuning is inert
    /// ([`Clip::tune_live`]); `None` (not analysed yet) follows whatever the clip plays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceId>,
    /// How the clip's pitch is cut into notes (the Vocal Editor's Notes selector). New tunings are
    /// [`NoteMode::NEW`] (Musical; the engine sets it when it makes one). A tuning saved without
    /// it (before modes existed) is [`NoteMode::Classic`], so it sounds as it did; Classic is not
    /// written.
    #[serde(default, skip_serializing_if = "is_classic")]
    pub note_mode: NoteMode,
    /// The engine's render-inputs hash of this tuning (`soundcraft_engine::vocal::render_inputs`;
    /// 0 when it has none), kept current after every command, undo, redo, install and load so the
    /// audio thread compares it with [`TunedSpans::inputs`] instead of hashing. Never saved.
    #[serde(skip)]
    pub fingerprint: u64,
}

impl PartialEq for VocalTune {
    fn eq(&self, o: &Self) -> bool {
        self.edits == o.edits
            && self.seg_ops == o.seg_ops
            && self.original == o.original
            && self.analysed == o.analysed
            && self.note_spans == o.note_spans
            && self.source == o.source
            && self.note_mode == o.note_mode
    }
}

fn is_classic(m: &NoteMode) -> bool {
    *m == NoteMode::Classic
}

impl VocalTune {
    /// How many notes have an edit that changes something.
    pub fn edited_count(&self) -> usize {
        self.edits.values().filter(|e| !e.is_identity()).count()
    }

    /// Every edit clamped to its ranges (non-finite values become 0); empty note spans dropped.
    pub fn sanitize(&mut self) {
        for e in self.edits.values_mut() {
            *e = std::mem::take(e).clamped();
        }
        self.note_spans.retain(|_, (start, end)| end > start);
    }
}

/// What an analysis was made from: the clip's source, the Clean Vocal alternate blended into it
/// (cleaned source, its origin in the original, the wet amount's `f32` bits) and the session rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnalysisKey {
    pub source: SourceId,
    /// `(cleaned source, origin, wet f32 bits)` while the clip blends in cleaned audio.
    pub clean: Option<(SourceId, Samples, u32)>,
    pub rate: u32,
}

impl AnalysisKey {
    /// A stable 64-bit FNV-1a hash of the key (saved in [`VocalTune::analysed`], so it must not
    /// depend on the Rust version, as `DefaultHasher` may).
    pub fn hash(&self) -> u64 {
        let mut h = Fnv::new();
        h.put(&self.source.0.to_le_bytes());
        match self.clean {
            None => h.put(&[0]),
            Some((src, origin, wet)) => {
                h.put(&[1]);
                h.put(&src.0.to_le_bytes());
                h.put(&origin.to_le_bytes());
                h.put(&wet.to_le_bytes());
            }
        }
        h.put(&self.rate.to_le_bytes());
        h.0
    }

    /// The key of what `clip` sounds like in `s` (its source blended with its Clean Vocal
    /// alternate at the wet amount, [`Clip::clean_alt`] and [`crate::CleanAlt::wet`]); `None` for
    /// MIDI and video clips. An alternate that plays nothing (wet 0: bypassed or amount 0) leaves
    /// the original exactly as it is, so it counts as no alternate.
    pub fn of(s: &Session, clip: &Clip) -> Option<AnalysisKey> {
        let source = clip.source()?;
        let clean = clip.clean_alt().filter(|k| k.wet() > 0.0).map(|k| (k.source, k.origin, k.wet().to_bits()));
        Some(AnalysisKey { source, clean, rate: s.sample_rate.hz() })
    }
}

struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn put(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// One clip analysis and the inputs it was made from.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipAnalysis {
    pub key: AnalysisKey,
    pub analysis: VocalAnalysis,
}

/// A clip's tuned audio: the re-rendered stretches of its source (everything else plays as it
/// did). `inputs` fingerprints what the render was made from (the engine's render-inputs hash),
/// `analysis` is the [`AnalysisKey::hash`] of the heard audio it was rendered from.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TunedSpans {
    pub inputs: u64,
    pub analysis: u64,
    /// Sorted by `start`, not overlapping.
    pub spans: Vec<TunedSpan>,
}

/// One re-rendered stretch: `channels[c][i]` replaces the source's sample `start + i`. Channel
/// data is shared (`Arc`): identical channels share one render, and spans a re-render leaves
/// alone are kept, not copied.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TunedSpan {
    /// Position in the source.
    pub start: Samples,
    pub channels: Vec<Arc<Vec<f32>>>,
}

impl TunedSpan {
    /// Length in samples (the shortest channel).
    pub fn len(&self) -> usize {
        self.channels.iter().map(|c| c.len()).min().unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Source position just past the span.
    pub fn end(&self) -> Samples {
        self.start.saturating_add(Samples::try_from(self.len()).unwrap_or(Samples::MAX))
    }

    /// The part over source positions `[a, b)` (clamped to the span); `None` when empty. The
    /// whole span keeps its data.
    fn part(&self, a: Samples, b: Samples) -> Option<TunedSpan> {
        let a = a.max(self.start);
        let b = b.min(self.end());
        if b <= a {
            return None;
        }
        if a == self.start && b == self.end() {
            return Some(self.clone());
        }
        let i0 = usize::try_from(a - self.start).ok()?;
        let i1 = usize::try_from(b - self.start).ok()?;
        // Channels sharing one render keep sharing their part: each distinct one is cut once.
        let mut channels: Vec<Arc<Vec<f32>>> = Vec::with_capacity(self.channels.len());
        for (k, c) in self.channels.iter().enumerate() {
            let earlier = self.channels.iter().take(k).position(|d| Arc::ptr_eq(c, d)).and_then(|j| channels.get(j)).map(Arc::clone);
            channels.push(earlier.unwrap_or_else(|| Arc::new(c.get(i0..i1).map(<[f32]>::to_vec).unwrap_or_default())));
        }
        Some(TunedSpan { start: a, channels })
    }
}

impl TunedSpans {
    /// `new` spliced over `base`: base spans a new span overlaps are trimmed to their parts
    /// outside it (as `soundcraft_vocal::resynth::splice` overwrites only the new span's samples),
    /// untouched base spans are kept as they are (their data shared), and the result is sorted.
    pub fn replace(base: Option<&TunedSpans>, inputs: u64, analysis: u64, new: Vec<TunedSpan>) -> TunedSpans {
        let new: Vec<TunedSpan> = new.into_iter().filter(|n| !n.is_empty()).collect();
        let mut out: Vec<TunedSpan> = Vec::new();
        for b in base.map_or(&[][..], |b| b.spans.as_slice()) {
            // The pieces of `b` no new span covers.
            let mut pieces = vec![(b.start, b.end())];
            for n in &new {
                let (ns, ne) = (n.start, n.end());
                pieces = pieces
                    .into_iter()
                    .flat_map(
                        |(a, e)| {
                            if ne <= a || ns >= e { vec![(a, e)] } else { [(a, ns), (ne, e)].into_iter().filter(|(x, y)| y > x).collect() }
                        },
                    )
                    .collect();
            }
            out.extend(pieces.into_iter().filter_map(|(a, e)| b.part(a, e)));
        }
        out.extend(new);
        out.sort_by_key(|s| s.start);
        TunedSpans { inputs, analysis, spans: out }
    }

    /// The tuned sample of channel `ch` (taken `ch % channels`) at source position `at`; `None`
    /// outside every span (the source plays there).
    pub fn sample_at(&self, ch: usize, at: Samples) -> Option<f32> {
        let i = self.spans.partition_point(|s| s.start <= at).checked_sub(1)?;
        let span = self.spans.get(i)?;
        let n = span.channels.len();
        if n == 0 {
            return None;
        }
        let k = usize::try_from(at.checked_sub(span.start)?).ok()?;
        span.channels.get(ch % n)?.get(k).copied()
    }
}

/// The derived vocal data (never saved; the engine carries it across undo and redo): analyses by
/// [`AnalysisKey::hash`] and tuned renders by clip. Behind `Arc`s, so session snapshots (undo,
/// the audio thread's copy) share them.
#[derive(Debug, Clone, Default)]
pub struct VocalCache {
    pub analyses: Arc<BTreeMap<u64, Arc<ClipAnalysis>>>,
    pub renders: Arc<BTreeMap<ClipId, Arc<TunedSpans>>>,
    /// The curves each clip's cached render was made from (a re-render only redoes the frames
    /// whose curves changed since).
    pub rendered_curves: Arc<BTreeMap<ClipId, Arc<EditCurves>>>,
    /// The heard audio (every channel) by [`AnalysisKey::hash`], kept so a slider drag does not
    /// read the source window again for every render.
    pub heard: Arc<BTreeMap<u64, Arc<Vec<Vec<f32>>>>>,
    /// Analysis keys and render inputs whose job failed (unsupported input): not offered again.
    pub failed: Arc<BTreeSet<u64>>,
    /// Why a failed job failed (its error text), when known.
    pub failed_why: Arc<BTreeMap<u64, String>>,
}

impl VocalCache {
    /// Whether `self` and `other` hold the very same maps (no copy needed between them).
    pub fn same(&self, other: &VocalCache) -> bool {
        Arc::ptr_eq(&self.analyses, &other.analyses)
            && Arc::ptr_eq(&self.renders, &other.renders)
            && Arc::ptr_eq(&self.rendered_curves, &other.rendered_curves)
            && Arc::ptr_eq(&self.heard, &other.heard)
            && Arc::ptr_eq(&self.failed, &other.failed)
            && Arc::ptr_eq(&self.failed_why, &other.failed_why)
    }
}

impl PartialEq for VocalCache {
    /// Derived data: two sessions are equal whatever their caches hold.
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChannelFormat, Clip, ClipId, Session, SourceId, TRACK_COLORS, Track, TrackId, TrackKind};
    use soundcraft_vocal::{NoteEdit, NoteId};

    #[test]
    fn names_preset_the_vocal_flag() {
        for n in ["Vox", "Lead Voc", "BGV 2", "vocals", "Lead Vocal", "Harmony", "VOX dbl", "Vox2"] {
            assert!(looks_vocal(n), "{n}");
        }
        for n in ["Audio 3", "Lead Guitar", "Kick", "Voxengo bus"] {
            assert!(!looks_vocal(n), "{n}");
        }
    }

    #[test]
    fn toggle_beats_the_name() {
        let mut vox = Track::new(TrackId(1), "Vox", TrackKind::Audio, ChannelFormat::Mono, TRACK_COLORS[0]);
        assert!(vox.is_vocal());
        vox.vocal = Some(false);
        assert!(!vox.is_vocal());
        let mut a = Track::new(TrackId(2), "Audio 1", TrackKind::Audio, ChannelFormat::Mono, TRACK_COLORS[0]);
        assert!(!a.is_vocal());
        a.vocal = Some(true);
        assert!(a.is_vocal());
        let mut m = Track::new(TrackId(3), "Vox", TrackKind::Midi, ChannelFormat::Mono, TRACK_COLORS[0]);
        assert!(!m.is_vocal());
        m.vocal = Some(true);
        assert!(!m.is_vocal(), "a MIDI track is never vocal");
    }

    #[test]
    fn old_sessions_load_without_tuning() {
        let c = Clip::audio(ClipId(5), "c", SourceId(2), 0, 0, 100);
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("tune").is_none(), "None omits the key");
        let back: Clip = serde_json::from_value(v).unwrap();
        assert!(back.tune.is_none());

        let t = Track::new(TrackId(1), "Vox", TrackKind::Audio, ChannelFormat::Mono, TRACK_COLORS[0]);
        let mut v = serde_json::to_value(&t).unwrap();
        assert!(v.get("vocal").is_none(), "None omits the key");
        v.as_object_mut().unwrap().remove("vocal");
        let back: Track = serde_json::from_value(v).unwrap();
        assert_eq!(back.vocal, None);

        let mut s = Session::default();
        let analysis = soundcraft_vocal::VocalAnalysis {
            track: soundcraft_vocal::PitchTrack { sample_rate: 48_000, hop_samples: 240, frames: Vec::new() },
            notes: Vec::new(),
            unvoiced: Vec::new(),
        };
        let key = AnalysisKey { source: SourceId(2), clean: None, rate: 48_000 };
        s.vocal.analyses = std::sync::Arc::new([(key.hash(), std::sync::Arc::new(ClipAnalysis { key, analysis }))].into_iter().collect());
        let json = serde_json::to_string(&s).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert!(back.vocal.analyses.is_empty(), "the cache is not saved");
        assert_eq!(back, s, "the cache is derived: sessions compare equal without it");
    }

    #[test]
    fn tune_round_trips_and_sanitizes() {
        let mut c = Clip::audio(ClipId(5), "c", SourceId(2), 0, 0, 100);
        let mut tune = VocalTune::default();
        tune.edits.insert(NoteId(120), NoteEdit { centre: 1.7, drift: f32::NAN, ..NoteEdit::default() });
        tune.note_spans.insert(NoteId(120), (120, 180));
        tune.note_spans.insert(NoteId(300), (300, 300));
        c.tune = Some(tune);
        c.sanitize();
        let spans = &c.tune_live().unwrap().note_spans;
        assert_eq!(spans.get(&NoteId(120)), Some(&(120, 180)));
        assert!(!spans.contains_key(&NoteId(300)), "sanitize drops empty spans");
        let e = &c.tune_live().unwrap().edits[&NoteId(120)];
        assert_eq!(e.centre, 1.0);
        assert_eq!(e.drift, 0.0);
        assert_eq!(c.tune_live().unwrap().edited_count(), 1);
        let back: Clip = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
        let v = serde_json::to_value(&c).unwrap();
        assert!(v["tune"].get("original").is_none(), "original skips when false");

        let mut m = Clip::midi(ClipId(6), "m", 0, 100, soundcraft_midi::Sequence::default());
        m.tune = Some(VocalTune::default());
        assert!(m.tune_live().is_none());
        m.sanitize();
        assert!(m.tune.is_none(), "sanitize drops tuning off MIDI clips");
    }

    #[test]
    fn tuned_spans_sample_lookup() {
        let spans = TunedSpans {
            inputs: 0,
            analysis: 0,
            spans: vec![
                TunedSpan { start: 100, channels: vec![Arc::new((0..10).map(|i| i as f32).collect())] },
                TunedSpan { start: 200, channels: vec![Arc::new(vec![0.5; 5]), Arc::new(vec![0.25; 5])] },
            ],
        };
        assert_eq!(spans.sample_at(0, 105), Some(5.0));
        assert_eq!(spans.sample_at(1, 105), Some(5.0), "channel ch % n");
        assert_eq!(spans.sample_at(0, 150), None);
        assert_eq!(spans.sample_at(0, 204), Some(0.5));
        assert_eq!(spans.sample_at(1, 204), Some(0.25));
        assert_eq!(spans.sample_at(0, 205), None);
        assert_eq!(spans.sample_at(0, -1), None);
        assert_eq!(spans.sample_at(0, 99), None);
        assert_eq!(TunedSpans::default().sample_at(0, 0), None);
    }

    fn span(start: Samples, v: Vec<f32>) -> TunedSpan {
        TunedSpan { start, channels: vec![Arc::new(v)] }
    }

    #[test]
    fn replace_splices_and_keeps_the_rest() {
        // A cached span over notes G (100..150) and S (150..200), and one far away.
        let gs: Vec<f32> = (100..200).map(|i| i as f32).collect();
        let far = span(500, vec![9.0; 10]);
        let base = TunedSpans { inputs: 1, analysis: 7, spans: vec![span(100, gs), far.clone()] };
        // A new render over S only (running a little past the cached span).
        let out = TunedSpans::replace(Some(&base), 2, 7, vec![span(150, vec![-1.0; 70])]);
        assert_eq!((out.inputs, out.analysis), (2, 7));
        for at in 100..150 {
            assert_eq!(out.sample_at(0, at), Some(at as f32), "G kept at {at}");
        }
        for at in 150..220 {
            assert_eq!(out.sample_at(0, at), Some(-1.0), "S replaced at {at}");
        }
        assert_eq!(out.sample_at(0, 220), None);
        assert_eq!(out.spans.len(), 3);
        assert!(Arc::ptr_eq(&out.spans[2].channels[0], &far.channels[0]), "untouched spans share their data");
        // A new span inside a base span keeps both ends.
        let out = TunedSpans::replace(Some(&base), 3, 7, vec![span(120, vec![0.0; 10])]);
        assert_eq!(out.sample_at(0, 119), Some(119.0));
        assert_eq!(out.sample_at(0, 125), Some(0.0));
        assert_eq!(out.sample_at(0, 130), Some(130.0));
        assert!(out.spans.windows(2).all(|w| w[0].end() <= w[1].start), "sorted, not overlapping");
        // Shared channel data stays shared in a trimmed part.
        let one = Arc::new((0..20).map(|i| i as f32).collect::<Vec<f32>>());
        let stereo = TunedSpans { inputs: 1, analysis: 7, spans: vec![TunedSpan { start: 0, channels: vec![Arc::clone(&one), one] }] };
        let out = TunedSpans::replace(Some(&stereo), 2, 7, vec![span(10, vec![0.0; 10])]);
        assert!(Arc::ptr_eq(&out.spans[0].channels[0], &out.spans[0].channels[1]), "one cut for both channels");
        assert_eq!(out.spans[0].len(), 10);
        // No base: just the new spans.
        assert_eq!(TunedSpans::replace(None, 4, 0, vec![span(10, vec![1.0; 3])]).spans.len(), 1);
    }
}
