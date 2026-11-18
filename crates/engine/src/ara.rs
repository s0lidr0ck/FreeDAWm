//! ARA 2 hosting, stage one: the session as ARA documents.
//!
//! Every ARA plug-in given to clips (Clip › ARA Plug-in…, `clip.ara_add`) gets one ARA document
//! per session ([`AraHost`]): the session is the document, each track holding such clips a region
//! sequence, each audio file they play an audio source, each clip an audio modification with one
//! playback region ([`graph`]). [`AraHost::sync`] runs on the UI thread (ARA's model thread),
//! after every change of the session (the engine's `revision`), so moves, trims, deletes, undo and
//! redo reach the plug-in in one editing cycle each; a clip that goes is kept by the plug-in,
//! deactivated, and comes back on undo. The plug-in reads the files' audio through the host on
//! its own threads, from the session's audio (memory or the disk cache, never the audio thread).
//!
//! Not yet (stage two): playback through the plug-in (the clip still plays its own audio),
//! bounce, freeze and saving the plug-in's document; stage three: its editor window. Clips that
//! play stretched, warped, reversed or looped are not given to the plug-in yet (ARA would need
//! the playback transformation), and say so.

use crate::Engine;
use soundcraft_ara_host::{
    AnalysisState, AudioSource, Document, FactoryPtr, Graph, HostEvent, RegionDesc, SequenceDesc, SourceDesc, sys::kARAContentTypeNotes,
};
use soundcraft_model::{Clip, ClipContent, ClipId, Session, SourceAudio, SourceId};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

/// Where the ARA log goes (`<settings>/logs/ara.log` in the app): every ARA document made or
/// refused, and every plug-in programming error ARA reports.
pub use soundcraft_ara_host::{log_file, recent_log, set_log_file};

/// A session file's audio as ARA reads it: the session's own decoded or disk-backed audio.
struct SessionAudio(Arc<SourceAudio>);

impl AudioSource for SessionAudio {
    fn channels(&self) -> usize {
        self.0.num_channels()
    }
    fn frames(&self) -> u64 {
        self.0.frames() as u64
    }
    fn sample_rate(&self) -> f64 {
        f64::from(self.0.sample_rate())
    }
    fn read(&self, ch: usize, start: u64, out: &mut [f32]) -> bool {
        // Blocking for a file played from disk (a reader thread may wait; the audio thread never
        // comes here).
        match usize::try_from(start) {
            Ok(s) => {
                self.0.read(ch, s, out);
                true
            }
            Err(_) => {
                out.iter_mut().for_each(|x| *x = 0.0);
                true
            }
        }
    }
}

/// Why a clip given to an ARA plug-in is not in its document, if it is not.
pub fn unsupported(clip: &Clip) -> Option<&'static str> {
    if !clip.is_audio() {
        return Some("ARA plug-ins take audio clips");
    }
    if (clip.stretch - 1.0).abs() > 1e-9 || !clip.warp.is_empty() || clip.reversed || clip.loop_len > 0 || clip.transpose_cents != 0 {
        return Some("a stretched, warped, reversed, looped or transposed clip is not given to ARA plug-ins yet");
    }
    None
}

/// The session's audio sources as ARA reads them, made once per file and kept while the
/// session's audio for it is the same (a new wrapper would read as a changed source).
#[derive(Default)]
pub struct AudioCache {
    map: HashMap<SourceId, (Arc<SourceAudio>, Arc<dyn AudioSource>)>,
}

impl AudioCache {
    fn get(&mut self, id: SourceId, audio: &Arc<SourceAudio>) -> Arc<dyn AudioSource> {
        if let Some((a, w)) = self.map.get(&id)
            && Arc::ptr_eq(a, audio)
        {
            return w.clone();
        }
        let w: Arc<dyn AudioSource> = Arc::new(SessionAudio(audio.clone()));
        self.map.insert(id, (audio.clone(), w.clone()));
        w
    }
}

/// The session's tempo map as ARA's sync points (seconds, quarter notes): one at every tempo
/// change (a ramp gets one every beat), one at quarter 0, and one four quarters past the last.
fn tempo_points(s: &Session) -> (Vec<(f64, f64)>, Vec<(i32, i32, f64)>) {
    use soundcraft_time::TICKS_PER_QUARTER;
    let sr = s.sample_rate;
    let hz = f64::from(sr.hz());
    let tempos = s.tempo.tempos();
    let mut ticks: BTreeSet<i64> = BTreeSet::from([0]);
    for (i, t) in tempos.iter().enumerate().take(4096) {
        ticks.insert(t.tick.max(0));
        if t.ramp
            && let Some(next) = tempos.get(i + 1)
        {
            let mut k = t.tick.max(0) + TICKS_PER_QUARTER;
            while k < next.tick && ticks.len() < 65_536 {
                ticks.insert(k);
                k += TICKS_PER_QUARTER;
            }
        }
    }
    let last = ticks.last().copied().unwrap_or(0);
    ticks.insert(last + 4 * TICKS_PER_QUARTER);
    let q = |t: i64| t as f64 / TICKS_PER_QUARTER as f64;
    let tempo = ticks.into_iter().map(|t| (s.tempo.ticks_f_to_samples_f(t as f64, sr) / hz, q(t))).collect();
    let bars = s.tempo.meters().iter().take(4096).map(|m| (m.numerator as i32, m.denominator as i32, q(m.tick.max(0)))).collect();
    (tempo, bars)
}

fn color([r, g, b]: [u8; 3]) -> [f32; 3] {
    [f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0]
}

/// The ARA document of `plugin` for the session: every audio clip (on the playlists that play)
/// given to it, with its track and file. Clips it cannot take yet come back with the reason.
pub fn graph(s: &Session, plugin: &str, audio: &mut AudioCache) -> (Graph, Vec<(ClipId, &'static str)>) {
    let mut g = Graph { name: s.name.clone(), ..Graph::default() };
    let mut skipped = Vec::new();
    let mut sources = BTreeSet::new();
    let hz = f64::from(s.sample_rate.hz());
    for (order, t) in s.tracks.iter().enumerate() {
        let mut on_track = false;
        for c in t.clips().iter().filter(|c| c.ara.as_ref().is_some_and(|a| a.plugin == plugin)) {
            if let Some(why) = unsupported(c) {
                skipped.push((c.id, why));
                continue;
            }
            let ClipContent::Audio { source, offset } = c.content else { continue };
            let (Some(file), Some(data)) = (s.source(source), s.pool.get(source)) else {
                skipped.push((c.id, "its audio file is missing"));
                continue;
            };
            if sources.insert(source) {
                g.sources.push(SourceDesc {
                    key: source.0,
                    name: file.name.clone(),
                    persistent_id: format!("source-{}", source.0),
                    audio: audio.get(source, data),
                });
            }
            let file_hz = f64::from(data.sample_rate().max(1));
            g.regions.push(RegionDesc {
                key: c.id.0,
                source: source.0,
                sequence: t.id.0,
                name: c.name.clone(),
                persistent_id: format!("clip-{}", c.id.0),
                start_in_source: offset.max(0) as f64 / file_hz,
                duration: c.length.max(1) as f64 / hz,
                start_in_song: c.start as f64 / hz,
                color: c.color.map(color),
            });
            on_track = true;
        }
        if on_track {
            g.sequences.push(SequenceDesc {
                key: t.id.0,
                name: t.name.clone(),
                order: i32::try_from(order).unwrap_or(i32::MAX),
                color: Some(color(t.color)),
            });
        }
    }
    (g.tempo, g.bars) = tempo_points(s);
    (g, skipped)
}

/// The plug-ins (ids) given to clips that play, with their names.
pub fn plugins_in_use(s: &Session) -> BTreeMap<String, String> {
    s.tracks.iter().flat_map(|t| t.clips()).filter_map(|c| c.ara.as_ref()).map(|a| (a.plugin.clone(), a.name.clone())).collect()
}

/// A plug-in's ARA factory and what keeps it valid (its binary and main factory object).
pub struct Factory {
    pub ptr: FactoryPtr,
    pub keep: Box<dyn std::any::Any>,
}

/// Where [`AraHost`] gets a plug-in's factory, by plug-in id.
pub type FactorySource = Box<dyn Fn(&str) -> Result<Factory, String>>;

/// The VST3 plug-ins' factories ([`soundcraft_vst3_host::ara_factory`]).
pub fn vst3_factories() -> FactorySource {
    Box::new(|id| soundcraft_vst3_host::ara_factory(id).map(|f| Factory { ptr: f.ptr(), keep: Box::new(f) }).map_err(|e| e.to_string()))
}

/// One plug-in's document and its factory (dropped in this order: the document first).
struct Doc {
    doc: Document,
    _factory: Factory,
    name: String,
    /// Notes the plug-in reports per audio source (counted when its content changes).
    notes: HashMap<u64, usize>,
    playback_asked: bool,
}

/// The session's ARA documents, one per plug-in in use. Lives on the UI thread (ARA's model
/// thread): it is neither `Send` nor `Sync`.
pub struct AraHost {
    factories: FactorySource,
    docs: BTreeMap<String, Doc>,
    /// Plug-ins that could not be given a document, with why.
    refused: BTreeMap<String, String>,
    /// Clips given to a plug-in but left out of its document, with why.
    skipped: HashMap<ClipId, &'static str>,
    audio: AudioCache,
    revision: Option<u64>,
}

impl Default for AraHost {
    fn default() -> Self {
        AraHost::new(vst3_factories())
    }
}

impl AraHost {
    pub fn new(factories: FactorySource) -> AraHost {
        AraHost { factories, docs: BTreeMap::new(), refused: BTreeMap::new(), skipped: HashMap::new(), audio: AudioCache::default(), revision: None }
    }

    /// Brings the documents in line with the session when it changed since the last call (one
    /// ARA editing cycle per document), then lets every plug-in send its notifications. Call it
    /// on the UI thread every frame or so. Returns whether any document changed.
    pub fn sync(&mut self, e: &Engine) -> bool {
        let mut changed = false;
        if self.revision != Some(e.revision) {
            self.revision = Some(e.revision);
            changed = self.rebuild(e.session());
        }
        self.idle();
        changed
    }

    fn rebuild(&mut self, s: &Session) -> bool {
        let used = plugins_in_use(s);
        let before = self.docs.len();
        self.docs.retain(|id, _| used.contains_key(id));
        self.refused.retain(|id, _| used.contains_key(id));
        let mut changed = self.docs.len() != before;
        self.skipped.clear();
        for (id, name) in &used {
            if !self.docs.contains_key(id) && !self.refused.contains_key(id) {
                let made = (self.factories)(id).and_then(|f| Document::new(f.ptr, &s.name).map(|d| (d, f)).map_err(|e| e.to_string()));
                match made {
                    Ok((doc, f)) => {
                        let shown = if doc.info().name.is_empty() { name.clone() } else { doc.info().name.clone() };
                        self.docs.insert(id.clone(), Doc { doc, _factory: f, name: shown, notes: HashMap::new(), playback_asked: false });
                    }
                    Err(why) => {
                        soundcraft_ara_host::log::line(&format!("{name} ({id}): no ARA document: {why}"));
                        self.refused.insert(id.clone(), why);
                    }
                }
            }
            let Some(d) = self.docs.get_mut(id) else { continue };
            let (g, skipped) = graph(s, id, &mut self.audio);
            self.skipped.extend(skipped);
            if d.doc.sync(&g).changed() {
                changed = true;
                for src in &g.sources {
                    count_notes(d, src.key);
                }
            }
        }
        changed
    }

    /// Lets each plug-in send its notifications (analysis progress, content changes).
    pub fn idle(&mut self) {
        for d in self.docs.values_mut() {
            for ev in d.doc.idle() {
                match ev {
                    HostEvent::SourceContentChanged(k) => count_notes(d, k),
                    HostEvent::Analysis { source, state: AnalysisState::Completed, .. } => count_notes(d, source),
                    HostEvent::PlaybackRequest(_) => d.playback_asked = true,
                    _ => {}
                }
            }
        }
    }

    /// One line about a clip's ARA plug-in, for the UI: what it has done with the clip, or why
    /// not. `None` for a clip without one.
    pub fn status(&self, clip: &Clip) -> Option<String> {
        let a = clip.ara.as_ref()?;
        let name = if a.name.is_empty() { a.plugin.as_str() } else { a.name.as_str() };
        if let Some(why) = self.refused.get(&a.plugin) {
            return Some(format!("{name}: cannot be used ({why})"));
        }
        if let Some(why) = self.skipped.get(&clip.id) {
            return Some(format!("{name}: not given this clip: {why}"));
        }
        let d = self.docs.get(&a.plugin)?;
        let src = clip.source()?.0;
        let notes = d.notes.get(&src).map(|n| format!("; it found {n} notes")).unwrap_or_default();
        Some(match d.doc.analysis(src) {
            Some((AnalysisState::Completed, _)) => format!("{}: analysed the clip's audio{notes}", d.name),
            Some((_, p)) => format!("{}: analysing the clip's audio ({:.0}%)", d.name, p * 100.0),
            None => format!("{}: has the clip's audio{notes}", d.name),
        })
    }

    /// One line per document, for the ARA window: the plug-in, what it holds and has read.
    pub fn summary(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .docs
            .values()
            .map(|d| {
                let (seqs, srcs, mods, _, kept) = d.doc.counts();
                let r = d.doc.read_stats();
                let mut line = format!(
                    "{} {} (ARA API generation {}): {mods} clip(s) on {seqs} track(s), {srcs} file(s); it read {} samples",
                    d.name,
                    d.doc.info().version,
                    d.doc.generation(),
                    r.samples
                );
                if kept > 0 {
                    line.push_str(&format!("; {kept} removed clip(s) kept for undo"));
                }
                if r.failed > 0 {
                    line.push_str(&format!("; {} read(s) failed", r.failed));
                }
                if d.playback_asked {
                    line.push_str("; it asked to control playback (not supported yet)");
                }
                line
            })
            .collect();
        out.extend(self.refused.iter().map(|(id, why)| format!("{id}: cannot be used: {why}")));
        out
    }

    /// The live documents' plug-in ids.
    pub fn documents(&self) -> Vec<String> {
        self.docs.keys().cloned().collect()
    }

    /// The notes a plug-in reports for an audio file of the session (`None`: none yet).
    pub fn notes(&self, plugin: &str, source: SourceId) -> Option<Vec<soundcraft_ara_host::Note>> {
        self.docs.get(plugin)?.doc.source_notes(source.0)
    }

    /// A plug-in's document's live objects (see [`Document::counts`]).
    pub fn counts(&self, plugin: &str) -> Option<(usize, usize, usize, usize, usize)> {
        self.docs.get(plugin).map(|d| d.doc.counts())
    }

    /// Why a plug-in has no document, if it was refused.
    pub fn refused(&self, plugin: &str) -> Option<&str> {
        self.refused.get(plugin).map(String::as_str)
    }

    /// Forgets the refusals, so the next change of the session tries those plug-ins again (after a
    /// rescan, say).
    pub fn retry(&mut self) {
        self.refused.clear();
        self.revision = None;
    }
}

fn count_notes(d: &mut Doc, source: u64) {
    if d.doc.source_content_available(source, kARAContentTypeNotes)
        && let Some(n) = d.doc.source_notes(source)
    {
        d.notes.insert(source, n.len());
    }
}
