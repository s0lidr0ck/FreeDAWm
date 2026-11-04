//! Vocal tuning's background work: what a clip sounds like (the heard audio the tuner analyses),
//! the session scale at a note's time, the job queue (`next_job` → [`VocalJob::run`] on a worker
//! or between frames → [`install`]) and keeping the clips' edits on their notes when an analysis
//! changes (a changed Clean Vocal Amount, a reload that analyses slightly differently).
//!
//! The engine spawns no threads (it stays wasm-clean): the host runs jobs where it can.

use crate::{Engine, EngineError, Result};
use serde::{Deserialize, Serialize};
use soundcraft_band::chord_theory::scale_offset;
use soundcraft_model::{AnalysisKey, Clip, ClipAnalysis, ClipId, Session, TrackId, TunedSpan, TunedSpans, VocalTune};
use soundcraft_time::{Range, Samples};
use soundcraft_vocal::resynth::{RenderedSpan, changed_frames_formant, render_spans_formant};
use soundcraft_vocal::{
    EditCurves, MAX_ANALYSIS_SECS, Note, NoteEdit, NoteId, NoteMode, PitchClassSet, PitchTrack, ScaleAt, SegOp, VocalAnalysis, VocalError,
    apply_seg_ops_with, build_curves_with, carry_edits, formant_curve, segment_with,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Why a job did not finish (re-exported for hosts that run jobs).
pub use soundcraft_vocal::VocalError as JobError;

/// A replayed `SegOp::Join` whose note moved in a re-analysis is pointed at the note starting
/// within this many frames (50 ms) of where it was.
const JOIN_SLACK_FRAMES: u32 = 10;

/// The whole source as `clip` hears it (Clean Vocal blended in at its amount, before gain, fades
/// and tuning), each of the source's channels; `None` for MIDI and video clips, missing audio, or a
/// source longer than an offline render holds ([`crate::io::render_limit`]`(2)`, as Commit).
pub fn heard_audio(s: &Session, clip: &Clip) -> Option<Vec<Vec<f32>>> {
    let frames = s.pool.get(clip.source()?)?.frames();
    if frames > crate::io::render_limit(2) {
        return None;
    }
    soundcraft_mix::clip_heard_window(s, clip, 0, frames)
}

/// The mean of `ch`'s channels (as long as the longest).
pub fn mono(ch: &[Vec<f32>]) -> Vec<f32> {
    let len = ch.iter().map(Vec::len).max().unwrap_or(0);
    let n = ch.len().max(1) as f32;
    (0..len).map(|i| ch.iter().filter_map(|c| c.get(i)).sum::<f32>() / n).collect()
}

/// A key name as `event.add_key_change` stores it: a tonic `A`–`G` with an optional `#`/`b`
/// (or `♯`/`♭`), then `major`/`maj`/`M`/nothing or `minor`/`min`/`m` (words in any case).
/// Returns `(tonic pitch class, scale)` with scale 0 major, 1 minor (the band's numbering).
pub fn parse_key(name: &str) -> Option<(u8, u8)> {
    let t = name.trim();
    let mut chars = t.chars();
    let base: i32 = match chars.next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest = chars.as_str();
    let (acc, rest) = if let Some(r) = rest.strip_prefix(['#', '♯']) {
        (1, r)
    } else if let Some(r) = rest.strip_prefix(['b', '♭']) {
        (-1, r)
    } else {
        (0, rest)
    };
    let mode = rest.trim();
    let scale = match mode {
        "" | "M" => 0,
        "m" => 1,
        _ => match mode.to_ascii_lowercase().as_str() {
            "major" | "maj" => 0,
            "minor" | "min" => 1,
            _ => return None,
        },
    };
    Some(((base + acc).rem_euclid(12) as u8, scale))
}

/// Whether the session sets a key to judge notes by: a key change that parses, band chords or
/// sections, or a band key other than the default (the Key ruler's rule).
pub fn has_key(s: &Session) -> bool {
    let d = soundcraft_model::BandState::default();
    s.key_signatures.iter().any(|(_, n)| parse_key(n).is_some())
        || !s.band.chords.is_empty()
        || !s.band.sections.is_empty()
        || (s.band.key, s.band.scale) != (d.key, d.scale)
}

/// The key in force at `at` as `(tonic pitch class, scale)`: the latest key change at or before
/// it that parses, else the band's key.
pub fn key_pc_at(s: &Session, at: Samples) -> (u8, u8) {
    s.key_signatures
        .iter()
        .filter(|(t, _)| *t <= at)
        .filter_map(|(t, n)| parse_key(n).map(|k| (*t, k)))
        .max_by_key(|(t, _)| *t)
        .map_or((s.band.key, s.band.scale), |(_, k)| k)
}

/// A key's scale tones as MIDI Cleanup reads the band key ([`crate::midi_cleanup::key_tones`]):
/// the seven degrees, and a minor key's raised 7th.
pub fn key_set(key: u8, scale: u8) -> PitchClassSet {
    let (key, scale) = (i32::from(key), i32::from(scale));
    let mut pcs: Vec<u8> = (0..7).map(|d| (key + scale_offset(scale, d)).rem_euclid(12) as u8).collect();
    if scale == 1 {
        pcs.push((key + 11).rem_euclid(12) as u8);
    }
    PitchClassSet::from_pcs(&pcs)
}

/// The session scale along one clip, for Snap to key: the latest key change
/// (`Session::key_signatures`) at or before the frame's session time, else the band's key.
/// Unparsable key changes are ignored.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionScale {
    /// Session time of source sample 0 (`clip.start − source_offset`).
    zero: Samples,
    /// Analysis hop (samples per frame).
    hop: u32,
    /// Key changes in time order.
    changes: Vec<(Samples, PitchClassSet)>,
    band: Option<PitchClassSet>,
}

impl ScaleAt for SessionScale {
    fn scale_at(&self, frame: u32) -> Option<PitchClassSet> {
        let at = self.zero.saturating_add(Samples::from(frame).saturating_mul(Samples::from(self.hop)));
        let k = self.changes.partition_point(|(t, _)| *t <= at);
        k.checked_sub(1).and_then(|i| self.changes.get(i)).map(|(_, set)| *set).or(self.band)
    }
}

/// The analysis hop at `rate` (5 ms, as the tracker uses).
fn hop_at(rate: u32) -> u32 {
    ((f64::from(rate) / 200.0).round() as u32).max(1)
}

/// [`SessionScale`] for `clip`: analysis frames map to session time through the clip's `start`
/// and `source_offset()`.
pub fn scale_at(s: &Session, clip: &Clip) -> SessionScale {
    let hop = AnalysisKey::of(s, clip)
        .and_then(|k| s.vocal.analyses.get(&k.hash()).map(|a| a.analysis.track.hop_samples))
        .filter(|h| *h > 0)
        .unwrap_or_else(|| hop_at(s.sample_rate.hz()));
    let changes = s.key_signatures.iter().filter_map(|(t, name)| parse_key(name).map(|(k, sc)| (*t, key_set(k, sc)))).collect();
    SessionScale { zero: clip.start.saturating_sub(clip.source_offset()), hop, changes, band: Some(key_set(s.band.key, s.band.scale)) }
}

/// 64-bit FNV-1a, for [`render_inputs`].
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

/// What `clip`'s tuned render is made from, hashed: the analysis key of the audio it hears, its
/// segmentation changes, its note edits, its note detection mode and the session scale at each
/// edited note (Snap to key).
/// `None` without a live tuning or with the tuning switched to Original. Never 0.
pub fn render_inputs(s: &Session, clip: &Clip) -> Option<u64> {
    let tune = clip.tune_live().filter(|t| !t.original)?;
    let key = AnalysisKey::of(s, clip)?;
    let mut h = Fnv::new();
    h.put(&key.hash().to_le_bytes());
    h.put(&serde_json::to_vec(&tune.seg_ops).unwrap_or_default());
    h.put(&[0xff]);
    h.put(&serde_json::to_vec(&tune.edits).unwrap_or_default());
    // Classic adds nothing, so Classic fingerprints are what they were before modes existed.
    if tune.note_mode != NoteMode::Classic {
        h.put(&[0xfe]);
        h.put(tune.note_mode.name().as_bytes());
    }
    // No clip formant adds nothing either (the notes' own shifts are in the edits).
    if tune.formant_st != 0.0 {
        h.put(&[0xfd]);
        h.put(&tune.formant_st.to_bits().to_le_bytes());
    }
    let scale = scale_at(s, clip);
    for id in tune.edits.keys() {
        match scale.scale_at(id.0) {
            Some(set) => {
                h.put(&[1]);
                h.put(&set.0.to_le_bytes());
            }
            None => h.put(&[0]),
        }
    }
    Some(h.0.max(1))
}

/// The active clips whose tuning's [`VocalTune::fingerprint`] is not [`render_inputs`] (0 for
/// none), with the value it should be.
fn stale_fingerprints(s: &Session) -> Vec<(ClipId, u64)> {
    s.tracks
        .iter()
        .flat_map(|t| t.clips())
        .filter_map(|c| {
            let t = c.tune.as_ref()?;
            let f = render_inputs(s, c).unwrap_or(0);
            (t.fingerprint != f).then_some((c.id, f))
        })
        .collect()
}

/// Brings every tuning's fingerprint up to date (the mix plays a render only when its inputs
/// equal the fingerprint). Copies the session only when one is stale.
pub(crate) fn refresh_fingerprints(doc: &mut Arc<Session>) {
    let stale = stale_fingerprints(doc);
    if !stale.is_empty() {
        apply_fingerprints(Arc::make_mut(doc), stale);
    }
}

/// [`refresh_fingerprints`] on a session not shared yet.
pub(crate) fn refresh_fingerprints_in(s: &mut Session) {
    let stale = stale_fingerprints(s);
    apply_fingerprints(s, stale);
}

fn apply_fingerprints(s: &mut Session, stale: Vec<(ClipId, u64)>) {
    for (id, f) in stale {
        if let Some(t) = s.find_clip_mut(id).and_then(|c| c.tune.as_mut()) {
            t.fingerprint = f;
        }
    }
}

/// Whether `clip`'s tuning cannot be rendered (its render job failed: a phrase over 60 s
/// without a breath), so it plays untuned.
pub fn render_failed(s: &Session, clip: &Clip) -> bool {
    render_inputs(s, clip).is_some_and(|i| s.vocal.failed.contains(&i))
}

/// What a clip whose tuning cannot be rendered because a phrase is too long reports.
pub const RENDER_FAILED: &str = "the tuning can't be rendered (a phrase runs over 60 s without a breath): the clip plays untuned";

/// What a failed render job reports, from its error text: the breath wording for an over-long
/// phrase, the error itself otherwise.
pub fn render_failure_text(why: &str) -> String {
    if why.contains("without a breath") || why.contains("no breath") {
        RENDER_FAILED.to_string()
    } else if why.is_empty() {
        "the tuning can't be rendered: the clip plays untuned".to_string()
    } else {
        format!("the tuning can't be rendered ({why}): the clip plays untuned")
    }
}

/// Why `clip`'s tuning plays untuned, when its render job failed.
pub fn render_failed_message(s: &Session, clip: &Clip) -> Option<String> {
    let i = render_inputs(s, clip)?;
    s.vocal.failed.contains(&i).then(|| render_failure_text(s.vocal.failed_why.get(&i).map_or("", String::as_str)))
}

/// Re-renders one clip's tuning: the frames whose curves changed since the cached render, spliced
/// over it.
#[derive(Debug, Clone)]
pub struct RenderJob {
    pub clip: ClipId,
    /// [`render_inputs`] of the clip when the job was made.
    pub inputs: u64,
    /// [`AnalysisKey::hash`] of the heard audio.
    pub analysis: u64,
    /// The heard audio, every channel, when cached (`VocalCache::heard`); otherwise [`run`]
    /// builds it from `session` on the worker.
    ///
    /// [`run`]: VocalJob::run
    pub audio: Option<Arc<Vec<Vec<f32>>>>,
    /// The session the job was made from (a shared snapshot: no copy).
    pub session: Arc<Session>,
    pub track: PitchTrack,
    pub curves: EditCurves,
    /// The formant shift per frame (semitones): the clip's and its notes' ([`formant_curve`]).
    pub formant: Vec<f32>,
    /// `changed_frames(curves of the cached render, curves)`.
    pub frames: Vec<std::ops::Range<u32>>,
    /// The cached render the new spans are spliced over (`None`: a full render).
    pub base: Option<Arc<TunedSpans>>,
}

/// A finished render, for [`install`].
#[derive(Debug, Clone)]
pub struct RenderDone {
    pub clip: ClipId,
    pub inputs: u64,
    pub spans: Arc<TunedSpans>,
    pub curves: Arc<EditCurves>,
    pub formant: Arc<Vec<f32>>,
    /// The heard audio it was made from, cached for the next render.
    pub audio: Arc<Vec<Vec<f32>>>,
}

/// The heard audio of `clip` in the snapshot `s` (worker side).
fn heard_in(s: &Session, clip: ClipId) -> std::result::Result<Vec<Vec<f32>>, VocalError> {
    s.find_clip(clip).and_then(|(_, c)| heard_audio(s, c)).ok_or_else(|| VocalError::Unsupported("the clip's audio is missing or too long".into()))
}

impl RenderJob {
    fn run(self, cancel: &AtomicBool) -> std::result::Result<RenderDone, VocalError> {
        let audio = match self.audio {
            Some(a) => a,
            None => Arc::new(heard_in(&self.session, self.clip)?),
        };
        let chans: &[Vec<f32>] = &audio;
        let first = chans.first().map(Vec::as_slice).unwrap_or(&[]);
        let n = chans.len().max(1);
        // Identical channels (or one): rendered once, the data shared.
        let same = chans.iter().skip(1).all(|c| c.len() == first.len() && c.iter().zip(first).all(|(a, b)| a.to_bits() == b.to_bits()));
        let new: Vec<TunedSpan> = if same {
            render_spans_formant(first, &self.track, &self.curves, &self.formant, &self.frames, cancel)?
                .into_iter()
                .map(|r| {
                    let data = Arc::new(r.samples);
                    TunedSpan { start: Samples::try_from(r.start).unwrap_or(Samples::MAX), channels: vec![data; n] }
                })
                .collect()
        } else {
            let per: Vec<Vec<RenderedSpan>> = chans
                .iter()
                .map(|c| render_spans_formant(c, &self.track, &self.curves, &self.formant, &self.frames, cancel))
                .collect::<std::result::Result<_, _>>()?;
            let count = per.first().map_or(0, Vec::len);
            let mut out = Vec::with_capacity(count);
            for i in 0..count {
                let parts: Vec<&RenderedSpan> = per.iter().filter_map(|p| p.get(i)).collect();
                let Some(head) = parts.first() else { continue };
                if parts.len() != per.len() || parts.iter().any(|p| p.start != head.start || p.samples.len() != head.samples.len()) {
                    return Err(VocalError::Unsupported("channels rendered different spans".into()));
                }
                out.push(TunedSpan {
                    start: Samples::try_from(head.start).unwrap_or(Samples::MAX),
                    channels: parts.iter().map(|p| Arc::new(p.samples.clone())).collect(),
                });
            }
            out
        };
        let spans = TunedSpans::replace(self.base.as_deref(), self.inputs, self.analysis, new);
        Ok(RenderDone {
            clip: self.clip,
            inputs: self.inputs,
            spans: Arc::new(spans),
            curves: Arc::new(self.curves),
            formant: Arc::new(self.formant),
            audio,
        })
    }
}

/// One piece of background work.
#[derive(Debug, Clone)]
pub enum VocalJob {
    /// Track and segment what `clip` sounds like in `session` (a shared snapshot; the heard audio
    /// is built on the worker), at `rate`: the clip audio behind `key`.
    Analyse { key: AnalysisKey, session: Arc<Session>, clip: ClipId, rate: u32 },
    /// Render one clip's tuning.
    Render(Box<RenderJob>),
}

/// A finished job, for [`install`].
#[derive(Debug, Clone)]
pub enum VocalDone {
    /// An analysis, and the heard audio it was made from (cached for the first render).
    Analysed(Arc<ClipAnalysis>, Option<Arc<Vec<Vec<f32>>>>),
    Rendered(Box<RenderDone>),
}

impl VocalJob {
    /// Does the work (worker thread, or between frames on wasm); stops promptly once `cancel`
    /// is set.
    pub fn run(self, cancel: &AtomicBool) -> std::result::Result<VocalDone, VocalError> {
        match self {
            VocalJob::Analyse { key, session, clip, rate } => {
                let audio = heard_in(&session, clip)?;
                drop(session);
                let analysis = soundcraft_vocal::analyse(&mono(&audio), rate, cancel)?;
                Ok(VocalDone::Analysed(Arc::new(ClipAnalysis { key, analysis }), Some(Arc::new(audio))))
            }
            VocalJob::Render(job) => Ok(VocalDone::Rendered(Box::new(job.run(cancel)?))),
        }
    }

    /// The clip a job is for, when it is one clip's (renders); analyses are shared by every clip
    /// hearing the same audio.
    pub fn clip(&self) -> Option<ClipId> {
        match self {
            VocalJob::Analyse { .. } => None,
            VocalJob::Render(job) => Some(job.clip),
        }
    }

    /// What [`mark_failed`] records when this job fails: the analysis key's hash, or the render
    /// inputs.
    pub fn failure_key(&self) -> u64 {
        match self {
            VocalJob::Analyse { key, .. } => key.hash(),
            VocalJob::Render(job) => job.inputs,
        }
    }
}

/// How far `clip` is from `playhead` (0 inside it).
fn distance(clip: &Clip, playhead: Samples) -> Samples {
    if playhead < clip.start { clip.start.saturating_sub(playhead) } else { playhead.saturating_sub(clip.end()).max(0) }
}

/// The next job, by priority: renders of clips whose tuning changed since their cached render
/// (`focus` first, then nearest `playhead`), then the analysis of `focus`, then analyses of clips
/// on Vocal tracks or with a live tuning, nearest `playhead` first. A clip whose analysis key is
/// already analysed is skipped, as are failed keys and render inputs. The host never asks while a
/// job is in flight, and cancels a running render when its clip's [`render_inputs`] change.
pub fn next_job(e: &Engine, focus: Option<ClipId>, playhead: Samples) -> Option<VocalJob> {
    pick(e, focus, playhead, None, &BTreeSet::new())
}

/// [`next_job`], restricted to `only` (any audio clip in it, vocal or not) and skipping the
/// analysis keys in `skip`.
fn pick(e: &Engine, focus: Option<ClipId>, playhead: Samples, only: Option<&[ClipId]>, skip: &BTreeSet<u64>) -> Option<VocalJob> {
    let snap = e.session_arc();
    let s = e.session();
    if let Some(job) = pick_render(&snap, focus, playhead, only, skip) {
        return Some(VocalJob::Render(Box::new(job)));
    }
    let mut cands: Vec<(bool, Samples, &Clip, AnalysisKey)> = Vec::new();
    for t in &s.tracks {
        for c in t.clips() {
            let wanted = match only {
                Some(ids) => ids.contains(&c.id),
                None => focus == Some(c.id) || t.is_vocal() || c.tune_live().is_some(),
            };
            if !wanted || !c.is_audio() {
                continue;
            }
            let Some(key) = AnalysisKey::of(s, c) else { continue };
            let h = key.hash();
            if s.vocal.analyses.contains_key(&h) || s.vocal.failed.contains(&h) || skip.contains(&h) {
                continue;
            }
            // Missing audio or longer than the tracker accepts: the job could only fail.
            if !analysable(s, c) {
                continue;
            }
            cands.push((focus != Some(c.id), distance(c, playhead), c, key));
        }
    }
    cands.sort_by_key(|(not_focus, d, c, _)| (*not_focus, *d, c.start, c.id));
    // The heard audio is built on the worker, from the snapshot: nothing heavy on this thread.
    cands.into_iter().next().map(|(_, _, c, key)| VocalJob::Analyse { key, session: Arc::clone(&snap), clip: c.id, rate: key.rate })
}

/// Whether `clip`'s analysis could run: its source audio is in the pool and no longer than the
/// tracker accepts ([`MAX_ANALYSIS_SECS`] at the session rate). [`next_job`] never offers a clip
/// that isn't, so the UI says it can't be analysed instead of waiting for it.
pub fn analysable(s: &Session, clip: &Clip) -> bool {
    let Some(audio) = clip.source().and_then(|src| s.pool.get(src)) else { return false };
    let frames = u64::try_from(audio.frames()).unwrap_or(u64::MAX);
    frames <= MAX_ANALYSIS_SECS.saturating_mul(u64::from(s.sample_rate.hz()))
}

/// Whether `clip` (audio, with no cached analysis) will never get one: it isn't
/// [`analysable`], or its analysis failed.
pub fn cannot_analyse(s: &Session, clip: &Clip) -> bool {
    if !clip.is_audio() {
        return false;
    }
    let Some(h) = AnalysisKey::of(s, clip).map(|k| k.hash()) else { return false };
    !s.vocal.analyses.contains_key(&h) && (s.vocal.failed.contains(&h) || !analysable(s, clip))
}

/// What the editor and the note commands say about a track whose clips [`cannot_analyse`].
pub fn cannot_analyse_text(track: &str) -> String {
    format!(
        "{track} can't be analysed (missing audio, or longer than {} min: Edit › Split into Files cuts a long recording into songs)",
        MAX_ANALYSIS_SECS / 60
    )
}

/// Whether `clip` plays stretched (`stretch != 1`): its tuning is not rendered or played.
pub fn stretched(clip: &Clip) -> bool {
    !clip.stretch.is_finite() || (clip.stretch - 1.0).abs() >= 1e-9
}

/// The render job of the clip that most needs one (see [`next_job`]).
fn pick_render(snap: &Arc<Session>, focus: Option<ClipId>, playhead: Samples, only: Option<&[ClipId]>, skip: &BTreeSet<u64>) -> Option<RenderJob> {
    let s: &Session = snap;
    let mut cands: Vec<(bool, Samples, &Clip)> = Vec::new();
    for t in &s.tracks {
        for c in t.clips() {
            if only.is_some_and(|ids| !ids.contains(&c.id)) || !c.is_audio() || stretched(c) {
                continue;
            }
            let Some(inputs) = render_inputs(s, c) else { continue };
            if s.vocal.renders.get(&c.id).is_some_and(|r| r.inputs == inputs) || s.vocal.failed.contains(&inputs) || skip.contains(&inputs) {
                continue;
            }
            cands.push((focus != Some(c.id), distance(c, playhead), c));
        }
    }
    cands.sort_by_key(|(not_focus, d, c)| (*not_focus, *d, c.start, c.id));
    cands.into_iter().find_map(|(_, _, c)| render_job(snap, c))
}

/// `clip`'s render job: the frames whose curves changed since the cached render, spliced over it.
/// After a re-analysis (another analysis key: a new Clean Vocal amount) the cached render came
/// from other audio, so the whole tuning is rendered afresh (no base, no previous curves). `None`
/// without a tuning to render or an analysis. The heard audio comes from the cache, or is built
/// on the worker from the snapshot `s`.
pub fn render_job(s: &Arc<Session>, clip: &Clip) -> Option<RenderJob> {
    let inputs = render_inputs(s, clip)?;
    let tune = clip.tune_live()?;
    let h = AnalysisKey::of(s, clip)?.hash();
    let a = analysis_for(s, clip)?;
    let curves = build_curves_with(&a, &tune.edits, &scale_at(s, clip), tune.note_mode);
    let formant = formant_curve(&a, &tune.edits, tune.formant_st);
    let base = s.vocal.renders.get(&clip.id).filter(|r| r.analysis == h);
    let prev = base.and_then(|_| s.vocal.rendered_curves.get(&clip.id));
    let base = prev.and(base);
    // A cached render without a formant curve was made with none.
    let prev_formant = s.vocal.rendered_formant.get(&clip.id).map_or(&[][..], |f| f.as_slice());
    let frames = changed_frames_formant(prev.map(|p| (&**p, prev_formant)), &curves, &formant);
    let audio = s.vocal.heard.get(&h).cloned();
    Some(RenderJob {
        clip: clip.id,
        inputs,
        analysis: h,
        audio,
        session: Arc::clone(s),
        track: a.track,
        curves,
        formant,
        frames,
        base: base.cloned(),
    })
}

/// The note detection mode of `clip`'s tuning; without one, the mode a new tuning would get
/// ([`NoteMode::NEW`]), so the notes shown before the first edit are the notes it edits.
pub fn note_mode(clip: &Clip) -> NoteMode {
    clip.tune_live().map_or(NoteMode::NEW, |t| t.note_mode)
}

/// `a` (a cached analysis: Classic notes) with its notes cut in `mode`. The pitch track is the
/// same in every mode, so a Musical clip needs no new analysis job.
pub fn in_mode(mut a: VocalAnalysis, mode: NoteMode) -> VocalAnalysis {
    if mode != NoteMode::Classic {
        (a.notes, a.unvoiced) = segment_with(&a.track, mode);
    }
    a
}

/// The cached analysis `clip` hears, its notes cut in its tuning's note detection mode and its
/// segmentation changes replayed.
pub fn analysis_for(s: &Session, clip: &Clip) -> Option<VocalAnalysis> {
    let key = AnalysisKey::of(s, clip)?;
    let a = s.vocal.analyses.get(&key.hash())?.analysis.clone();
    let mode = note_mode(clip);
    let mut a = in_mode(a, mode);
    if let Some(t) = clip.tune_live() {
        apply_seg_ops_with(&mut a, &t.seg_ops, mode);
    }
    Some(a)
}

/// Records the frames of every edited note of `a` (an analysis with `tune`'s segmentation
/// replayed) in `tune.note_spans`, so the edits can be matched by overlap after a re-analysis.
/// Edits on notes `a` doesn't have keep no span. Call it whenever the edits or the analysis change.
pub fn sync_note_spans(tune: &mut VocalTune, a: &VocalAnalysis) {
    let by_id: BTreeMap<NoteId, (u32, u32)> = a.notes.iter().map(|n| (n.id, (n.start, n.end))).collect();
    tune.note_spans = tune.edits.keys().filter_map(|id| by_id.get(id).map(|sp| (*id, *sp))).collect();
}

/// A bare note over `[start, end)` (only the frames matter to [`carry_edits`]).
fn span_note(id: NoteId, (start, end): (u32, u32)) -> Note {
    Note { id, start, end, scoop: None, centre_cents: 0.0, level_db: 0.0, drift_cents: 0.0, vibrato_cents: 0.0 }
}

/// `tune` moved onto `new` (a fresh analysis, segmentation not replayed): every edit re-matched
/// by overlap from its saved frames, `SegOp::Join` ids pointed at the moved notes, the spans and
/// `analysed`/`source` brought up to date. `old` is the analysis the edits were made on, when
/// still cached, to find the frames of edits saved without them.
fn retune(tune: &VocalTune, old: Option<&ClipAnalysis>, new: &ClipAnalysis) -> VocalTune {
    let mut t = tune.clone();
    // Edits saved without frames (made before spans were kept): take them from the analysis the
    // edits were made on.
    let mode = t.note_mode;
    if let Some(old) = old {
        let mut a = in_mode(old.analysis.clone(), mode);
        apply_seg_ops_with(&mut a, &t.seg_ops, mode);
        for n in &a.notes {
            if t.edits.contains_key(&n.id) {
                t.note_spans.entry(n.id).or_insert((n.start, n.end));
            }
        }
    }
    // A join names its second note exactly: follow it when it moved a little.
    let fresh = in_mode(new.analysis.clone(), mode);
    let raw = &fresh.notes;
    for op in &mut t.seg_ops {
        if let SegOp::Join { at } = op
            && !raw.iter().any(|n| n.id == NoteId(*at))
            && let Some(n) = raw.iter().skip(1).filter(|n| n.start.abs_diff(*at) <= JOIN_SLACK_FRAMES).min_by_key(|n| n.start.abs_diff(*at))
        {
            *at = n.id.0;
        }
    }
    let mut a = fresh.clone();
    apply_seg_ops_with(&mut a, &t.seg_ops, mode);
    // Still no frames (nothing cached to read them from): the ids are kept as they are.
    let (spanned, unspanned): (BTreeMap<NoteId, NoteEdit>, BTreeMap<NoteId, NoteEdit>) =
        std::mem::take(&mut t.edits).into_iter().partition(|(id, _)| t.note_spans.contains_key(id));
    let old_notes: Vec<Note> = t.note_spans.iter().filter(|(id, _)| spanned.contains_key(id)).map(|(id, sp)| span_note(*id, *sp)).collect();
    let mut edits = carry_edits(&old_notes, &spanned, &a.notes);
    for (id, e) in unspanned {
        edits.entry(id).or_insert(e);
    }
    t.edits = edits;
    sync_note_spans(&mut t, &a);
    t.analysed = new.key.hash();
    t.source.get_or_insert(new.key.source);
    t
}

/// Installs a finished job (no undo step; the revision bumps). An analysis is cached under its
/// key (analyses no clip's current key uses are dropped), and every clip with a live tuning that
/// hears it gets its edits moved onto it ([`carry_edits`] by overlap with the saved note frames),
/// which marks the session changed. `false` when the result matches no clip any more.
///
/// A render is stored only while it still matches the clip's tuning (`render_inputs(clip) ==
/// Some(done.inputs)`); otherwise it is dropped (`false`) and the next [`next_job`] renders again.
pub fn install(e: &mut Engine, done: VocalDone) -> bool {
    let ok = match done {
        VocalDone::Analysed(a, audio) => install_analysis(e, a, audio),
        VocalDone::Rendered(r) => install_render(e, *r),
    };
    e.refresh_vocal_fingerprints();
    ok
}

fn install_render(e: &mut Engine, d: RenderDone) -> bool {
    let s = e.session();
    let Some((_, c)) = s.find_clip(d.clip) else { return false };
    if render_inputs(s, c) != Some(d.inputs) {
        return false;
    }
    let used: BTreeSet<u64> = s.all_clips().filter_map(|c| AnalysisKey::of(s, c)).map(|k| k.hash()).collect();
    let mut renders = (*s.vocal.renders).clone();
    let mut curves = (*s.vocal.rendered_curves).clone();
    let mut formants = (*s.vocal.rendered_formant).clone();
    let mut heard: BTreeMap<u64, Arc<Vec<Vec<f32>>>> =
        s.vocal.heard.iter().filter(|(k, _)| used.contains(k)).map(|(k, v)| (*k, Arc::clone(v))).collect();
    heard.insert(d.spans.analysis, d.audio);
    renders.insert(d.clip, d.spans);
    curves.insert(d.clip, d.curves);
    formants.insert(d.clip, d.formant);
    let v = &mut e.session_mut().vocal;
    v.renders = Arc::new(renders);
    v.rendered_curves = Arc::new(curves);
    v.rendered_formant = Arc::new(formants);
    v.heard = Arc::new(heard);
    true
}

fn install_analysis(e: &mut Engine, a: Arc<ClipAnalysis>, audio: Option<Arc<Vec<Vec<f32>>>>) -> bool {
    let h = a.key.hash();
    let s = e.session();
    let used: BTreeSet<u64> = s.all_clips().filter_map(|c| AnalysisKey::of(s, c)).map(|k| k.hash()).collect();
    let cache = &s.vocal.analyses;
    if !used.contains(&h) {
        if cache.keys().any(|k| !used.contains(k)) {
            let kept: BTreeMap<u64, Arc<ClipAnalysis>> = cache.iter().filter(|(k, _)| used.contains(k)).map(|(k, v)| (*k, Arc::clone(v))).collect();
            e.session_mut().vocal.analyses = Arc::new(kept);
        }
        return false;
    }
    // The edits' own analysis: the one they say they refer to, else what this audio was analysed
    // as before, else any analysis of the same source.
    let old_for = |t: &VocalTune| {
        cache.get(&t.analysed).or_else(|| cache.get(&h)).or_else(|| cache.values().find(|c| c.key.source == a.key.source)).map(Arc::clone)
    };
    let mut retuned: Vec<(ClipId, VocalTune)> = Vec::new();
    for c in s.all_clips() {
        let Some(t) = c.tune_live() else { continue };
        if AnalysisKey::of(s, c).map(|k| k.hash()) != Some(h) {
            continue;
        }
        let old = old_for(t);
        let nt = retune(t, old.as_deref(), &a);
        if &nt != t {
            retuned.push((c.id, nt));
        }
    }
    let mut map: BTreeMap<u64, Arc<ClipAnalysis>> = cache.iter().filter(|(k, _)| used.contains(k)).map(|(k, v)| (*k, Arc::clone(v))).collect();
    map.insert(h, a);
    // The heard audio the analysis was made from: the first render reuses it.
    // Kept only while a clip of this audio is tuned: otherwise it is dead weight (a 4-minute
    // stereo take is ~92 MB), and the first render builds it on the worker.
    let tuned = s.all_clips().any(|c| c.tune_live().is_some() && AnalysisKey::of(s, c).map(|k| k.hash()) == Some(h));
    let heard = audio.filter(|_| tuned).map(|x| {
        let mut m: BTreeMap<u64, Arc<Vec<Vec<f32>>>> =
            s.vocal.heard.iter().filter(|(k, _)| used.contains(k)).map(|(k, v)| (*k, Arc::clone(v))).collect();
        m.insert(h, x);
        m
    });
    let s = e.session_mut();
    s.vocal.analyses = Arc::new(map);
    if let Some(m) = heard {
        s.vocal.heard = Arc::new(m);
    }
    for (id, t) in retuned {
        if let Some(c) = s.find_clip_mut(id) {
            c.tune = Some(t);
        }
    }
    true
}

/// Runs jobs (renders and analyses) for the clips in `only` until none is left (blocking: for
/// commands, the CLI and the control channel). Returns how many ran. An analysis that cannot be
/// made is an error; a render that cannot be made (a phrase over 60 s) is marked failed and the
/// clip plays untuned ([`render_failed`]).
pub fn run_until_idle(e: &mut Engine, only: &[ClipId]) -> Result<usize> {
    let mut n = 0;
    let cancel = AtomicBool::new(false);
    let none = BTreeSet::new();
    while let Some(job) = pick(e, None, 0, Some(only), &none) {
        let key = job.failure_key();
        let render = job.clip().is_some();
        let done = match job.run(&cancel) {
            Ok(done) => done,
            Err(err) => {
                if err != VocalError::Cancelled {
                    mark_failed_because(e, key, &err.to_string());
                }
                if render {
                    n += 1;
                    continue;
                }
                return Err(EngineError::Unsupported(format!("vocal analysis: {err}")));
            }
        };
        // An install that matches no clip any more: don't run it again. Safe here only because
        // this loop runs inside one command, so nothing can change the session between the job
        // and its install; the background host never does this (an undo may need the key again).
        if !install(e, done) {
            mark_failed(e, key);
        }
        n += 1;
    }
    Ok(n)
}

/// Drops the derived vocal data no clip uses any more: analyses and heard audio of analysis keys
/// no clip (on any playlist) hears, and renders of clips without a live tuning. Called on load
/// only: after Commit or Remove the cache is kept, so their Undo plays tuned again at once. `true`
/// when anything was dropped.
pub fn prune(s: &mut Session) -> bool {
    let used: BTreeSet<u64> = s.all_clips().filter_map(|c| AnalysisKey::of(s, c)).map(|k| k.hash()).collect();
    let tuned: BTreeSet<ClipId> = s.all_clips().filter(|c| c.tune_live().is_some()).map(|c| c.id).collect();
    let v = &mut s.vocal;
    let mut changed = false;
    if v.analyses.keys().any(|k| !used.contains(k)) {
        v.analyses = Arc::new(v.analyses.iter().filter(|(k, _)| used.contains(k)).map(|(k, a)| (*k, Arc::clone(a))).collect());
        changed = true;
    }
    if v.heard.keys().any(|k| !used.contains(k)) {
        v.heard = Arc::new(v.heard.iter().filter(|(k, _)| used.contains(k)).map(|(k, a)| (*k, Arc::clone(a))).collect());
        changed = true;
    }
    if v.renders.keys().any(|c| !tuned.contains(c)) {
        v.renders = Arc::new(v.renders.iter().filter(|(c, _)| tuned.contains(c)).map(|(c, r)| (*c, Arc::clone(r))).collect());
        changed = true;
    }
    if v.rendered_curves.keys().any(|c| !tuned.contains(c)) {
        v.rendered_curves = Arc::new(v.rendered_curves.iter().filter(|(c, _)| tuned.contains(c)).map(|(c, r)| (*c, Arc::clone(r))).collect());
        changed = true;
    }
    if v.rendered_formant.keys().any(|c| !tuned.contains(c)) {
        v.rendered_formant = Arc::new(v.rendered_formant.iter().filter(|(c, _)| tuned.contains(c)).map(|(c, r)| (*c, Arc::clone(r))).collect());
        changed = true;
    }
    changed
}

/// Records that the job for analysis key or render inputs `key_hash` failed (anything but a
/// cancel), so [`next_job`] no longer offers it. Derived data: no undo step, kept across Undo.
pub fn mark_failed(e: &mut Engine, key_hash: u64) {
    mark_failed_because(e, key_hash, "");
}

/// [`mark_failed`] with the reason (the job's error text), which [`render_failed_message`]
/// reports.
pub fn mark_failed_because(e: &mut Engine, key_hash: u64, why: &str) {
    let v = &e.session().vocal;
    if v.failed.contains(&key_hash) && (why.is_empty() || v.failed_why.contains_key(&key_hash)) {
        return;
    }
    let mut set = (*v.failed).clone();
    set.insert(key_hash);
    let mut whys = (*v.failed_why).clone();
    if !why.is_empty() {
        whys.insert(key_hash, why.to_string());
    }
    let v = &mut e.session_mut().vocal;
    v.failed = Arc::new(set);
    v.failed_why = Arc::new(whys);
}

/// One note of one clip: what the note commands take (`{"clip": id, "note": start frame}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NoteRef {
    pub clip: ClipId,
    pub note: NoteId,
}

/// Where `clip`'s analysis frames sit: session time of source sample 0 and the hop.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameMap {
    zero: Samples,
    hop: Samples,
    /// The clip's window in source samples.
    window: (Samples, Samples),
}

impl FrameMap {
    pub(crate) fn of(clip: &Clip, a: &VocalAnalysis) -> FrameMap {
        let offset = clip.source_offset();
        FrameMap {
            zero: clip.start.saturating_sub(offset),
            hop: Samples::from(a.track.hop_samples.max(1)),
            window: (offset, offset.saturating_add(clip.length)),
        }
    }

    /// Session time of frame `f`.
    pub(crate) fn time(&self, f: u32) -> Samples {
        self.zero.saturating_add(Samples::from(f).saturating_mul(self.hop))
    }

    /// The frame sounding at session time `at` (`None` before the source starts).
    pub(crate) fn frame(&self, at: Samples) -> Option<u32> {
        let rel = at.checked_sub(self.zero)?;
        if rel < 0 {
            return None;
        }
        u32::try_from(rel / self.hop).ok()
    }

    /// Whether frames `[start, end)` overlap the clip's window.
    pub(crate) fn in_window(&self, start: u32, end: u32) -> bool {
        let s = Samples::from(start).saturating_mul(self.hop);
        let e = Samples::from(end).saturating_mul(self.hop);
        s < self.window.1 && e > self.window.0
    }
}

/// `clip`'s pitch-shift and gain curves under its live tuning; `None` without a live tuning or an
/// analysis.
pub fn curves_for(s: &Session, clip: &Clip) -> Option<EditCurves> {
    let tune = clip.tune_live()?;
    let a = analysis_for(s, clip)?;
    Some(build_curves_with(&a, &tune.edits, &scale_at(s, clip), tune.note_mode))
}

/// The note sounding at session time `at` on `track`'s active playlist (an analysed audio clip
/// under `at`, the note whose frames hold it).
pub fn note_at(s: &Session, track: TrackId, at: Samples) -> Option<NoteRef> {
    let t = s.track(track)?;
    t.clips().iter().filter(|c| c.is_audio() && c.range().contains(at)).find_map(|c| {
        let a = analysis_for(s, c)?;
        let f = FrameMap::of(c, &a).frame(at)?;
        a.notes.iter().find(|n| n.start <= f && f < n.end).map(|n| NoteRef { clip: c.id, note: n.id })
    })
}

/// Every note of every analysed audio clip on `track` (inside each clip's window), limited to
/// those overlapping `range` when given: Select all.
pub fn track_notes(s: &Session, track: TrackId, range: Option<Range>) -> Vec<NoteRef> {
    let Some(t) = s.track(track) else { return Vec::new() };
    let mut out = Vec::new();
    for c in t.clips().iter().filter(|c| c.is_audio()) {
        let Some(a) = analysis_for(s, c) else { continue };
        let m = FrameMap::of(c, &a);
        for n in &a.notes {
            let span = Range::new(m.time(n.start), m.time(n.end));
            if m.in_window(n.start, n.end) && range.is_none_or(|r| span.overlaps(&r)) {
                out.push(NoteRef { clip: c.id, note: n.id });
            }
        }
    }
    out
}
