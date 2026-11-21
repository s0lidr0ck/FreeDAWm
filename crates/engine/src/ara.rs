//! ARA 2 hosting: the session as ARA documents, clips heard through their plug-in, the plug-in's
//! document saved with the session.
//!
//! Every ARA plug-in given to clips (`clip.ara_add`) gets one ARA document per session
//! ([`AraHost`]): the session is the document, each track holding such clips a region sequence,
//! each audio file they play an audio source, each clip an audio modification with one playback
//! region ([`graph`]). Clips on several tracks given to the same plug-in are in its one document,
//! each track its own region sequence (plug-ins that align a dub to a guide see both).
//! [`AraHost::sync`] runs on the UI thread (ARA's model thread) after every change of the session
//! (the engine's `revision`), so moves, trims, splits, duplicates, deletes, undo and redo reach the
//! plug-in in one editing cycle each; a clip that goes is kept by the plug-in, deactivated, and
//! comes back on undo; a duplicated or split clip's modification starts as a clone of the one it
//! was copied from (`ClipAra::origin`).
//!
//! **Playback.** Each clip given to a plug-in (and not bypassed) gets its own playback renderer
//! instance (an "ARA voice": the plug-in's VST3 instance bound to its document with the playback
//! and editor renderer roles, given the clip's playback region, then activated, all here on the
//! model thread). The voices go on the realtime engine's [`AraBoard`]; the audio thread renders
//! the clip through its voice instead of reading the file, at the transport position read ahead
//! by the plug-in's latency, and applies clip gain and fades to the result (see
//! `soundcraft_mix::ara`). A voice is taken off the board and destroyed here before its playback
//! region goes. A plug-in that cannot be bound, gives no playback renderer or renders something
//! that is not a number is bypassed: the clip plays its own audio and [`AraHost::status`] says why.
//!
//! **Offline.** [`AraHost::prepare_offline`] (called before every bounce, freeze, commit, save…)
//! renders each such clip through an instance set up for offline rendering into the session's
//! `ara_renders`; every offline render plays those in place of the files, so a bounce sounds as
//! playback did.
//!
//! **Saving.** [`AraHost::store_archives`] stores each plug-in's document with ARA's archiving
//! (`storeObjectsToArchive`) into the session (`Session::ara_archives`); a document made for a
//! session that has one restores it in its first editing cycle (`restoreObjectsFromArchive`) when
//! the plug-in's factory says it can read it (its document archive id or a compatible one). The
//! archive of a plug-in that is not installed (or cannot read it) is kept as it is.
//!
//! **Undo.** The plug-in keeps its own undo history for edits made in it; those are not on
//! FreeDAWm's undo list. FreeDAWm's own undo and redo reach the plug-in as one editing cycle
//! (`beginEditing` … `endEditing`) each, so the document stays in step with the session.

use crate::Engine;
use soundcraft_ara_host::sys::{
    ARADocumentControllerRef, kARAContentTypeNotes, kARAEditorRendererRole, kARAEditorViewRole, kARAPlaybackRendererRole,
    kARAPlaybackTransformationNoChanges, kARAPlaybackTransformationTimestretch,
};
use soundcraft_ara_host::{
    AnalysisState, AudioSource, BoundInstance, Document, FactoryPtr, Graph, HostEvent, RawExtension, RegionDesc, SequenceDesc, SourceDesc,
};
use soundcraft_dsp::Plugin;
use soundcraft_mix::ara::{AraBoard, AraVoice};
use soundcraft_model::{AraArchive, AraRender, AraRenders, Clip, ClipContent, ClipId, Session, SourceAudio, SourceId, ara_fingerprint};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, PoisonError};

/// Where the ARA log goes (`<settings>/logs/ara.log` in the app): every ARA document made or
/// refused, and every plug-in programming error ARA reports.
pub use soundcraft_ara_host::PlaybackRequest;
pub use soundcraft_ara_host::log::line as log_line;
pub use soundcraft_ara_host::{log_file, recent_log, set_log_file};

/// The roles FreeDAWm assigns (and so knows): playback and editor rendering per clip, the editor
/// view on the instance that shows the plug-in's window.
pub const KNOWN_ROLES: i32 = kARAPlaybackRendererRole | kARAEditorRendererRole | kARAEditorViewRole;
/// A clip's voice: playback and editor renderer of its one playback region.
pub const VOICE_ROLES: i32 = kARAPlaybackRendererRole | kARAEditorRendererRole;
/// Block size of offline renders.
const OFFLINE_BLOCK: usize = 1024;

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

/// Why a clip given to an ARA plug-in is not in its document, if it is not. `transformations`:
/// what the plug-in supports (`kARAPlaybackTransformation*`): a time-stretched clip is offered to
/// one that supports time-stretch.
pub fn unsupported(clip: &Clip, transformations: i32) -> Option<&'static str> {
    if !clip.is_audio() {
        return Some("ARA plug-ins take audio clips");
    }
    if !clip.warp.is_empty() || clip.reversed || clip.loop_len > 0 || clip.transpose_cents != 0 {
        return Some("a warped, reversed, looped or transposed clip cannot be given to an ARA plug-in (ARA cannot describe it)");
    }
    if (clip.stretch - 1.0).abs() > 1e-9 && transformations & kARAPlaybackTransformationTimestretch == 0 {
        return Some("a stretched clip cannot be given to this ARA plug-in: it does not support time-stretched playback regions");
    }
    if !(clip.stretch.is_finite() && clip.stretch > 0.0) {
        return Some("the clip's stretch is not a usable number");
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
/// given to it, with its track and file. Clips it cannot take come back with the reason.
/// `transformations`: what the plug-in supports.
pub fn graph(s: &Session, plugin: &str, transformations: i32, audio: &mut AudioCache) -> (Graph, Vec<(ClipId, &'static str)>) {
    let mut g = Graph { name: s.name.clone(), ..Graph::default() };
    let mut skipped = Vec::new();
    let mut sources = BTreeSet::new();
    let hz = f64::from(s.sample_rate.hz());
    for (order, t) in s.tracks.iter().enumerate() {
        let mut on_track = false;
        for c in t.clips().iter().filter(|c| c.ara.as_ref().is_some_and(|a| a.plugin == plugin)) {
            if let Some(why) = unsupported(c, transformations) {
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
            let length = c.length.max(1);
            let span = c.source_span(length).max(1);
            // The file's frames over the session's rate: equal for a plain clip of a file at the
            // session's rate; otherwise ARA's time-stretch (only offered when supported).
            let duration = length as f64 / hz;
            let source_duration = span as f64 / file_hz;
            let stretched = (duration - source_duration).abs() > 0.5 / hz.max(file_hz);
            g.regions.push(RegionDesc {
                key: c.id.0,
                source: source.0,
                sequence: t.id.0,
                name: c.name.clone(),
                persistent_id: format!("clip-{}", c.id.0),
                start_in_source: offset.max(0) as f64 / file_hz,
                duration,
                source_duration: if stretched { source_duration } else { duration },
                transformation: if stretched { kARAPlaybackTransformationTimestretch } else { kARAPlaybackTransformationNoChanges },
                origin: c.ara.as_ref().and_then(|a| a.origin),
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

/// Where [`AraHost`] gets a plug-in instance bound to a document: (plug-in id, the document
/// controller, known roles, assigned roles) → the instance, **unprepared**, and the extension its
/// binding gave.
pub type InstanceSource = Box<dyn Fn(&str, ARADocumentControllerRef, i32, i32) -> Result<(Box<dyn Plugin>, RawExtension), String>>;

/// The VST3 plug-ins' factories ([`soundcraft_vst3_host::ara_factory`]).
pub fn vst3_factories() -> FactorySource {
    Box::new(|id| soundcraft_vst3_host::ara_factory(id).map(|f| Factory { ptr: f.ptr(), keep: Box::new(f) }).map_err(|e| e.to_string()))
}

/// The VST3 plug-ins' instances ([`soundcraft_vst3_host::instantiate_ara`]).
pub fn vst3_instances() -> InstanceSource {
    Box::new(|id, dc, known, assigned| {
        soundcraft_vst3_host::instantiate_ara(id, dc, known, assigned).map(|(p, e)| (Box::new(p) as Box<dyn Plugin>, e)).map_err(|e| e.to_string())
    })
}

/// Voices whose instance is gone but whose `Arc` the audio thread may still hold: dropped here,
/// on the model thread, once it no longer does (so nothing is freed on the audio thread).
fn graveyard() -> &'static Mutex<Vec<Arc<AraVoice>>> {
    static G: Mutex<Vec<Arc<AraVoice>>> = Mutex::new(Vec::new());
    &G
}

fn sweep_graveyard() {
    graveyard().lock().unwrap_or_else(PoisonError::into_inner).retain(|v| Arc::strong_count(v) > 1);
}

/// One clip's playback renderer instance on the realtime engine's board.
struct Voice {
    /// Kept for its `Drop`: destroys the instance, here on the model thread.
    _bound: BoundInstance<dyn Plugin>,
    voice: Arc<AraVoice>,
    board: Arc<AraBoard>,
    clip: u64,
    /// The audio source its region plays (a clip that now plays another file gets a new region).
    source: u64,
    /// What it was made for: channels, (rate, block).
    ch: usize,
    audio: (u32, usize),
}

impl Drop for Voice {
    fn drop(&mut self) {
        // Off the board first (the audio thread stops starting blocks with it), then the instance
        // is destroyed (`bound`, after this: it waits for a block being rendered).
        if let Some(v) = self.board.remove(self.clip)
            && !Arc::ptr_eq(&v, &self.voice)
        {
            self.board.set(self.clip, v);
        }
        graveyard().lock().unwrap_or_else(PoisonError::into_inner).push(self.voice.clone());
    }
}

/// One plug-in's document and its factory (dropped in this order: the voices, the document, the
/// factory).
struct Doc {
    voices: HashMap<u64, Voice>,
    /// The instance showing the plug-in's editor (bound with the editor view role), once opened.
    editor: Option<EditorView>,
    doc: Document,
    _factory: Factory,
    name: String,
    /// Notes the plug-in reports per audio source (counted when its content changes).
    notes: HashMap<u64, usize>,
    /// Clips whose voice could not be made, with why (tried again when the clip changes).
    voice_failed: HashMap<u64, (u64, String)>,
    /// A number per clip bumped whenever the plug-in says its audio changed: an offline render
    /// made before is stale.
    content: HashMap<u64, u64>,
    /// The offline renders made: clip → (fingerprint, content number).
    rendered: HashMap<u64, (u64, u64)>,
    /// What restoring the saved document did (shown once).
    restore_note: Option<String>,
    /// The playback requests the plug-in made, for the app.
    requests: Vec<PlaybackRequest>,
}

/// The plug-in instance whose window shows the plug-in's editor: bound with the editor view role,
/// told the Edit window's selection (dropped in this order: the window, then the instance).
struct EditorView {
    editor: Box<dyn soundcraft_dsp::PluginEditor>,
    bound: BoundInstance<dyn Plugin>,
    /// What it was last told (a hash of the selection and the hidden tracks).
    told: Option<u64>,
}

impl Doc {
    fn bump(&mut self, clip: u64) {
        *self.content.entry(clip).or_default() += 1;
    }
}

/// The session's ARA documents, one per plug-in in use. Lives on the UI thread (ARA's model
/// thread): it is neither `Send` nor `Sync`.
pub struct AraHost {
    factories: FactorySource,
    instances: Option<InstanceSource>,
    docs: BTreeMap<String, Doc>,
    /// Plug-ins that could not be given a document, with why.
    refused: BTreeMap<String, String>,
    /// Clips given to a plug-in but left out of its document, with why.
    skipped: HashMap<ClipId, &'static str>,
    audio: AudioCache,
    revision: Option<u64>,
    /// The realtime engine's board and the rate and block size it plays at.
    board: Option<Arc<AraBoard>>,
    rate: Option<(u32, usize)>,
    /// Session data changed in a plug-in since the last call to [`AraHost::take_unsaved`].
    unsaved: bool,
    /// Something a plug-in did that the app shows once ([`AraHost::take_messages`]).
    messages: Vec<String>,
}

impl Default for AraHost {
    fn default() -> Self {
        AraHost::with_instances(vst3_factories(), vst3_instances())
    }
}

impl AraHost {
    /// A host that makes documents but no plug-in instances (nothing is heard through ARA).
    pub fn new(factories: FactorySource) -> AraHost {
        AraHost {
            factories,
            instances: None,
            docs: BTreeMap::new(),
            refused: BTreeMap::new(),
            skipped: HashMap::new(),
            audio: AudioCache::default(),
            revision: None,
            board: None,
            rate: None,
            unsaved: false,
            messages: Vec::new(),
        }
    }

    /// A host that also makes plug-in instances: clips are heard through their plug-in.
    pub fn with_instances(factories: FactorySource, instances: InstanceSource) -> AraHost {
        AraHost { instances: Some(instances), ..AraHost::new(factories) }
    }

    /// The realtime engine's board and the rate and block size it plays at: from now on each clip
    /// given to a plug-in gets a voice there. `None` takes every voice off.
    pub fn set_realtime(&mut self, board: Option<Arc<AraBoard>>, rate: f32, max_block: usize) {
        let audio = Some((rate.max(1.0).round() as u32, max_block.max(1)));
        let same_board = match (&self.board, &board) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        if same_board && self.rate == audio {
            return;
        }
        for d in self.docs.values_mut() {
            d.voices.clear();
            d.voice_failed.clear();
        }
        self.board = board;
        self.rate = audio;
        self.revision = None;
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
        self.follow_selection(e.session());
        // While an editor is open the voices render when stopped too (the plug-in's previews).
        if let Some(b) = &self.board {
            b.set_preview(self.docs.values().any(|d| d.editor.as_ref().is_some_and(|v| v.editor.is_open())));
        }
        self.idle();
        sweep_graveyard();
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
                    Ok((mut doc, f)) => {
                        let shown = if doc.info().name.is_empty() { name.clone() } else { doc.info().name.clone() };
                        let restore_note = restore_saved(&mut doc, s, id, &shown);
                        self.docs.insert(
                            id.clone(),
                            Doc {
                                voices: HashMap::new(),
                                editor: None,
                                doc,
                                _factory: f,
                                name: shown,
                                notes: HashMap::new(),
                                voice_failed: HashMap::new(),
                                content: HashMap::new(),
                                rendered: HashMap::new(),
                                restore_note,
                                requests: Vec::new(),
                            },
                        );
                    }
                    Err(why) => {
                        soundcraft_ara_host::log::line(&format!("{name} ({id}): no ARA document: {why}"));
                        self.refused.insert(id.clone(), why);
                    }
                }
            }
            let Some(d) = self.docs.get_mut(id) else { continue };
            let flags = d.doc.info().transformations;
            let (g, skipped) = graph(s, id, flags, &mut self.audio);
            self.skipped.extend(skipped);
            // Voices whose clip went, was bypassed or changed track width go before the document
            // drops their playback region.
            let wanted: HashMap<u64, usize> = voice_clips(s, id, &g);
            let sources: HashMap<u64, u64> = g.regions.iter().map(|r| (r.key, r.source)).collect();
            d.voices.retain(|k, v| wanted.get(k).is_some_and(|ch| *ch == v.ch && Some(v.audio) == self.rate) && sources.get(k) == Some(&v.source));
            d.voice_failed.retain(|k, (fp, _)| wanted.contains_key(k) && clip_fingerprint(s, *k) == Some(*fp));
            if d.doc.sync(&g).changed() {
                changed = true;
                for src in &g.sources {
                    count_notes(d, src.key);
                }
            }
            if let Some(r) = d.doc.take_restore_result() {
                let note = match r {
                    Ok(()) => format!("{}: its saved edits were restored", d.name),
                    Err(why) => format!("{}: {why}; its saved state is kept in the session", d.name),
                };
                self.messages.push(note.clone());
                d.restore_note = Some(note);
            }
            if let (Some(board), Some(rate), Some(make)) = (&self.board, self.rate, &self.instances) {
                for (clip, ch) in &wanted {
                    if d.voices.contains_key(clip) || d.voice_failed.contains_key(clip) {
                        continue;
                    }
                    let source = sources.get(clip).copied().unwrap_or(0);
                    match make_voice(make, id, d, (*clip, source), *ch, rate, board) {
                        Ok(v) => {
                            d.voices.insert(*clip, v);
                        }
                        Err(why) => {
                            soundcraft_ara_host::log::line(&format!("{}: clip {clip} plays its own audio: {why}", d.name));
                            let fp = clip_fingerprint(s, *clip).unwrap_or(0);
                            d.voice_failed.insert(*clip, (fp, why));
                        }
                    }
                }
            }
        }
        changed
    }

    /// Lets each plug-in send its notifications (analysis progress, content changes, playback
    /// requests).
    pub fn idle(&mut self) {
        for d in self.docs.values_mut() {
            for ev in d.doc.idle() {
                match ev {
                    HostEvent::SourceContentChanged(k) => {
                        count_notes(d, k);
                        // A source's content (its analysis) rarely changes what plays, but any
                        // render of its clips may be stale.
                        for c in d.doc.regions() {
                            d.bump(c);
                        }
                    }
                    HostEvent::Analysis { source, state: AnalysisState::Completed, .. } => count_notes(d, source),
                    HostEvent::RegionContentChanged(k) => d.bump(k),
                    HostEvent::DocumentDataChanged => self.unsaved = true,
                    HostEvent::PlaybackRequest(r) => {
                        soundcraft_ara_host::log::line(&format!("{}: asked to {}", d.name, r.describe()));
                        d.requests.push(r);
                    }
                    HostEvent::Analysis { .. } => {}
                }
            }
        }
    }

    /// The playback requests the plug-ins made since the last call, oldest first.
    pub fn take_requests(&mut self) -> Vec<PlaybackRequest> {
        self.docs.values_mut().flat_map(|d| std::mem::take(&mut d.requests)).collect()
    }

    /// Whether a plug-in changed its document since the last call (the session wants saving).
    pub fn take_unsaved(&mut self) -> bool {
        std::mem::take(&mut self.unsaved)
    }

    /// What plug-ins did that the app shows once (restored saved edits, could not).
    pub fn take_messages(&mut self) -> Vec<String> {
        std::mem::take(&mut self.messages)
    }

    /// Renders every clip given to a plug-in through an instance set up for offline rendering
    /// (its playback region, the transport at each block, read ahead by its latency), unless its
    /// last render still fits it, and puts the renders on the session: bounces, freezes, commits,
    /// and consolidates then hear the plug-in exactly as playback does. Call it on this
    /// thread before any offline render. Returns how many clips were rendered.
    pub fn prepare_offline(&mut self, e: &mut Engine) -> usize {
        let Some(make) = &self.instances else { return 0 };
        let s = e.session_arc();
        let rate = s.sample_rate.hz();
        let mut renders: BTreeMap<ClipId, Arc<AraRender>> = s.ara_renders.0.as_ref().clone();
        let mut made = 0;
        for (id, d) in &mut self.docs {
            let flags = d.doc.info().transformations;
            let (g, _) = graph(&s, id, flags, &mut self.audio);
            for (clip, ch) in voice_clips(&s, id, &g) {
                let Some(c) = s.find_clip(ClipId(clip)).map(|(_, c)| c) else { continue };
                let Some(fp) = ara_fingerprint(c) else { continue };
                let content = d.content.get(&clip).copied().unwrap_or(0);
                let fits = renders.get(&ClipId(clip)).is_some_and(|r| r.fingerprint == fp);
                if fits && d.rendered.get(&clip) == Some(&(fp, content)) {
                    continue;
                }
                match render_clip_offline(make, id, d, &s, c, ch, rate) {
                    Ok(audio) => {
                        renders.insert(ClipId(clip), Arc::new(AraRender { fingerprint: fp, audio: Arc::new(audio) }));
                        d.rendered.insert(clip, (fp, content));
                        made += 1;
                    }
                    Err(why) => {
                        soundcraft_ara_host::log::line(&format!("{}: offline render of clip {clip} failed: {why}; it renders its own audio", d.name));
                        renders.remove(&ClipId(clip));
                        d.rendered.remove(&clip);
                    }
                }
            }
        }
        // Renders of clips no longer given to a plug-in are of no use.
        renders.retain(|k, _| s.find_clip(*k).is_some_and(|(_, c)| ara_fingerprint(c).is_some()));
        e.set_ara_renders(AraRenders(Arc::new(renders)));
        made
    }

    /// Stores each plug-in's document into the session (`Session::ara_archives`), replacing the
    /// archive it had with the same document archive id and keeping the others (plug-ins not
    /// installed, archives of another version). Call before saving (no undo step).
    pub fn store_archives(&mut self, e: &mut Engine) {
        let mut archives = e.session().ara_archives.clone();
        let mut any = false;
        for (id, d) in &mut self.docs {
            let info = d.doc.info().clone();
            let old_view = archives.iter().find(|a| a.plugin == *id && a.archive_id == info.document_archive_id).map(|a| a.view_state.clone());
            let view_state = match d.editor.as_ref().and_then(|e| e.bound.cell().with(|p| p.save_state()).flatten()) {
                Some(v) => soundcraft_model::b64::encode(&v),
                None => old_view.unwrap_or_default(),
            };
            match d.doc.store() {
                Ok(data) => {
                    archives.retain(|a| !(a.plugin == *id && a.archive_id == info.document_archive_id));
                    archives.push(AraArchive {
                        plugin: id.clone(),
                        name: d.name.clone(),
                        version: info.version.clone(),
                        archive_id: info.document_archive_id.clone(),
                        data: soundcraft_model::b64::encode(&data),
                        view_state,
                    });
                    any = true;
                }
                Err(why) => soundcraft_ara_host::log::line(&format!("{}: its document was not saved: {why}", d.name)),
            }
        }
        if any && archives != e.session().ara_archives {
            e.set_ara_archives(archives);
        }
    }

    /// One line about a clip's ARA plug-in, for the UI: what it does with the clip, or why not.
    /// `None` for a clip without one.
    pub fn status(&self, clip: &Clip) -> Option<String> {
        let a = clip.ara.as_ref()?;
        let name = if a.name.is_empty() { a.plugin.as_str() } else { a.name.as_str() };
        if let Some(why) = self.refused.get(&a.plugin) {
            return Some(format!("{name}: cannot be used ({why}); the clip plays its own audio and the plug-in's saved edits are kept"));
        }
        if let Some(why) = self.skipped.get(&clip.id) {
            return Some(format!("{name}: not given this clip: {why}"));
        }
        let d = self.docs.get(&a.plugin)?;
        if a.bypass {
            return Some(format!("{}: bypassed (the clip plays its own audio; the plug-in keeps its edits)", d.name));
        }
        let src = clip.source()?.0;
        let notes = d.notes.get(&src).map(|n| format!("; it found {n} notes")).unwrap_or_default();
        let what = match d.doc.analysis(src) {
            Some((AnalysisState::Completed, _)) => format!("analysed the clip's audio{notes}"),
            Some((_, p)) => format!("analysing the clip's audio ({:.0}%)", p * 100.0),
            None => format!("has the clip's audio{notes}"),
        };
        let heard = match (d.voices.get(&clip.id.0), d.voice_failed.get(&clip.id.0)) {
            (Some(v), _) if v.voice.failed() => "bypassed: it rendered audio that is not a number".to_string(),
            (Some(_), _) => "the clip plays through the plug-in".to_string(),
            (None, Some((_, why))) => format!("bypassed: {why}"),
            (None, None) if self.instances.is_none() || self.board.is_none() => "heard when the audio engine runs".to_string(),
            (None, None) => "the clip plays its own audio for now".to_string(),
        };
        Some(format!("{}: {what}; {heard}", d.name))
    }

    /// Whether `clip` plays through its plug-in now (it has a live voice that has not failed).
    pub fn plays_through(&self, clip: &Clip) -> bool {
        clip.ara.as_ref().and_then(|a| self.docs.get(&a.plugin)).and_then(|d| d.voices.get(&clip.id.0)).is_some_and(|v| !v.voice.failed())
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
                    "{} {} (ARA API generation {}): {mods} clip(s) on {seqs} track(s), {srcs} file(s); it read {} samples; {} clip(s) play through it",
                    d.name,
                    d.doc.info().version,
                    d.doc.generation(),
                    r.samples,
                    d.voices.values().filter(|v| !v.voice.failed()).count()
                );
                if kept > 0 {
                    line.push_str(&format!("; {kept} removed clip(s) kept for undo"));
                }
                if r.failed > 0 {
                    line.push_str(&format!("; {} read(s) failed", r.failed));
                }
                if let Some(n) = &d.restore_note {
                    line.push_str(&format!("; {n}"));
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

    /// The clips that have a voice (play through their plug-in), by plug-in.
    pub fn voices(&self, plugin: &str) -> Vec<u64> {
        self.docs.get(plugin).map(|d| d.voices.keys().copied().collect()).unwrap_or_default()
    }

    /// Why a plug-in has no document, if it was refused.
    pub fn refused(&self, plugin: &str) -> Option<&str> {
        self.refused.get(plugin).map(String::as_str)
    }

    /// Whether the plug-in's editor window can be opened for `clip`, or why not.
    pub fn can_open_editor(&self, clip: &Clip) -> Result<(), String> {
        let a = clip.ara.as_ref().ok_or("The clip is not given to an ARA plug-in")?;
        if let Some(why) = self.refused.get(&a.plugin) {
            return Err(format!("The plug-in cannot be used: {why}"));
        }
        if let Some(why) = self.skipped.get(&clip.id) {
            return Err(format!("The plug-in was not given this clip: {why}"));
        }
        if self.instances.is_none() {
            return Err("Plug-in windows are not available here".into());
        }
        if !self.docs.contains_key(&a.plugin) {
            return Err("The plug-in's document is being made: try again in a moment".into());
        }
        Ok(())
    }

    /// Opens (or raises) the editor window of `clip`'s plug-in: an instance bound to its document
    /// with the editor view role (made the first time, its saved window state restored), told what
    /// the Edit window has selected. Fails with why.
    pub fn open_editor(&mut self, s: &Session, clip: ClipId) -> Result<(), String> {
        let c = s.find_clip(clip).map(|(_, c)| c).ok_or("No such clip")?;
        self.can_open_editor(c)?;
        let plugin = c.ara.as_ref().map(|a| a.plugin.clone()).unwrap_or_default();
        let make = self.instances.as_ref().ok_or("Plug-in windows are not available here")?;
        let d = self.docs.get_mut(&plugin).ok_or("The plug-in has no document")?;
        if d.editor.is_none() {
            let (p, raw) = make(&plugin, d.doc.controller_ref(), KNOWN_ROLES, kARAEditorViewRole)?;
            let bound = BoundInstance::new(p, raw, kARAEditorViewRole, &d.name)?;
            if !bound.has_view() {
                return Err(format!("{} gave no ARA editor view", d.name));
            }
            let state = s
                .ara_archives
                .iter()
                .find(|a| a.plugin == plugin && !a.view_state.is_empty())
                .and_then(|a| soundcraft_model::b64::decode(&a.view_state));
            let editor = bound
                .cell()
                .with(|p| {
                    if let Some(st) = &state
                        && !p.load_state(st)
                    {
                        soundcraft_ara_host::log::line("the plug-in refused its saved editor state");
                    }
                    p.editor()
                })
                .flatten()
                .ok_or_else(|| format!("{} has no editor window", d.name))?;
            soundcraft_ara_host::log::line(&format!("{}: editor view instance made", d.name));
            d.editor = Some(EditorView { editor, bound, told: None });
        }
        let Some(ev) = d.editor.as_mut() else { return Err("No editor".into()) };
        ev.editor.open().inspect_err(|e| soundcraft_ara_host::log::line(&format!("{}: its editor window did not open: {e}", d.name)))?;
        ev.told = None;
        self.follow_selection(s);
        Ok(())
    }

    /// Whether `plugin`'s editor window is open.
    pub fn editor_open(&self, plugin: &str) -> bool {
        self.docs.get(plugin).and_then(|d| d.editor.as_ref()).is_some_and(|e| e.editor.is_open())
    }

    /// Keeps open editor windows serviced and told the Edit window's selection when it changes:
    /// the selected clips given to the plug-in (playback regions), the selected tracks holding its
    /// clips (region sequences), the selected time range, and the tracks hidden in the Edit window.
    pub fn follow_selection(&mut self, s: &Session) {
        let hz = f64::from(s.sample_rate.hz().max(1));
        let sel = &s.edit.selection;
        let range = (sel.end > sel.start).then(|| (sel.start as f64 / hz, (sel.end - sel.start) as f64 / hz));
        for d in self.docs.values_mut() {
            let Some(ev) = d.editor.as_mut() else { continue };
            let _ = ev.editor.idle();
            if !ev.editor.is_open() {
                continue;
            }
            let regions: Vec<u64> = s.edit.selected_clips.iter().map(|c| c.0).filter(|k| d.doc.region_ref(*k).is_some()).collect();
            let seqs: Vec<u64> = s.edit.selected_tracks.iter().map(|t| t.0).filter(|k| d.doc.sequence_ref(*k).is_some()).collect();
            let hidden: Vec<u64> = s.tracks.iter().filter(|t| t.hidden).map(|t| t.id.0).filter(|k| d.doc.sequence_ref(*k).is_some()).collect();
            let key = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                (&regions, &seqs, &hidden, range.map(|(a, b)| (a.to_bits(), b.to_bits()))).hash(&mut h);
                h.finish()
            };
            if ev.told == Some(key) {
                continue;
            }
            ev.told = Some(key);
            let r: Vec<_> = regions.iter().filter_map(|k| d.doc.region_ref(*k)).collect();
            let q: Vec<_> = seqs.iter().filter_map(|k| d.doc.sequence_ref(*k)).collect();
            let hq: Vec<_> = hidden.iter().filter_map(|k| d.doc.sequence_ref(*k)).collect();
            ev.bound.notify_selection(&r, &q, range);
            ev.bound.hide_sequences(&hq);
        }
    }

    /// Forgets the refusals and failed voices, so the next change of the session tries those
    /// plug-ins again (after a rescan, say).
    pub fn retry(&mut self) {
        self.refused.clear();
        for d in self.docs.values_mut() {
            d.voice_failed.clear();
        }
        self.revision = None;
    }
}

/// Restores the session's saved document of plug-in `id` into a new document, when there is one
/// the plug-in can read. A note when there is one it cannot.
fn restore_saved(doc: &mut Document, s: &Session, id: &str, name: &str) -> Option<String> {
    let mine: Vec<&AraArchive> = s.ara_archives.iter().filter(|a| a.plugin == id).collect();
    if mine.is_empty() {
        return None;
    }
    let info = doc.info();
    let readable = |a: &&&AraArchive| a.archive_id == info.document_archive_id || info.compatible_archive_ids.contains(&a.archive_id);
    match mine.iter().find(readable) {
        Some(a) => match a.bytes() {
            Some(bytes) => {
                soundcraft_ara_host::log::line(&format!("{name}: restoring its saved document ({} bytes, archive id {})", bytes.len(), a.archive_id));
                let archive_id = a.archive_id.clone();
                doc.restore_next_sync(bytes, &archive_id);
                None
            }
            None => Some(format!("{name}: its saved document is damaged and was not restored (it is kept in the session)")),
        },
        None => {
            let ids: Vec<&str> = mine.iter().map(|a| a.archive_id.as_str()).collect();
            let note = format!(
                "{name}: this version cannot read the saved document (archive {}); it starts fresh and the saved one is kept in the session",
                ids.join(", ")
            );
            soundcraft_ara_host::log::line(&note);
            Some(note)
        }
    }
}

/// The clips of plug-in `id` that should have a voice (in its document, not bypassed), with
/// their track's channel count.
fn voice_clips(s: &Session, id: &str, g: &Graph) -> HashMap<u64, usize> {
    let main = soundcraft_mix::main_channels(s);
    let regions: BTreeSet<u64> = g.regions.iter().map(|r| r.key).collect();
    let mut out = HashMap::new();
    for t in &s.tracks {
        for c in t.clips().iter().filter(|c| regions.contains(&c.id.0)) {
            if c.ara.as_ref().is_some_and(|a| a.plugin == id && !a.bypass) {
                out.insert(c.id.0, soundcraft_mix::strip_channels(t, main).clamp(1, soundcraft_mix::MAX_CHANNELS));
            }
        }
    }
    out
}

fn clip_fingerprint(s: &Session, clip: u64) -> Option<u64> {
    s.find_clip(ClipId(clip)).and_then(|(_, c)| ara_fingerprint(c))
}

/// Makes a bound instance with the clip's playback region, set up (`prepare`) for `ch` channels at
/// `rate`, offline or not.
fn bound_instance(
    make: &InstanceSource,
    id: &str,
    d: &Doc,
    clip: u64,
    ch: usize,
    (rate, block): (u32, usize),
    offline: bool,
) -> Result<BoundInstance<dyn Plugin>, String> {
    let region = d.doc.region_ref(clip).ok_or("the plug-in has no playback region for the clip")?;
    let (plugin, raw) = make(id, d.doc.controller_ref(), KNOWN_ROLES, VOICE_ROLES)?;
    let bound = BoundInstance::new(plugin, raw, VOICE_ROLES, &d.name)?;
    if !bound.has_playback() {
        return Err("the plug-in gave no playback renderer".into());
    }
    // Regions are given while the instance is not active, then it is activated.
    bound.add_region(region);
    bound.cell().with(|p| {
        p.set_offline(offline);
        p.prepare(rate as f32, block, ch);
    });
    Ok(bound)
}

fn make_voice(
    make: &InstanceSource,
    id: &str,
    d: &Doc,
    (clip, source): (u64, u64),
    ch: usize,
    rate: (u32, usize),
    board: &Arc<AraBoard>,
) -> Result<Voice, String> {
    let bound = bound_instance(make, id, d, clip, ch, rate, false)?;
    let latency = bound.cell().with(|p| p.latency()).unwrap_or(0);
    let voice = Arc::new(AraVoice::new(bound.cell(), latency));
    if let Some(old) = board.set(clip, voice.clone()) {
        graveyard().lock().unwrap_or_else(PoisonError::into_inner).push(old);
    }
    soundcraft_ara_host::log::line(&format!("{}: clip {clip} plays through the plug-in ({ch} channel(s), latency {latency})", d.name));
    Ok(Voice { _bound: bound, voice, board: board.clone(), clip, source, ch, audio: rate })
}

/// The clip's audio through an offline instance, from its start for its length, before clip gain
/// and fades.
fn render_clip_offline(make: &InstanceSource, id: &str, d: &Doc, s: &Session, c: &Clip, ch: usize, rate: u32) -> Result<Vec<Vec<f32>>, String> {
    let bound = bound_instance(make, id, d, c.id.0, ch, (rate, OFFLINE_BLOCK), true)?;
    let len = usize::try_from(c.length.max(0)).map_err(|_| "the clip is too long")?;
    let mut out = vec![Vec::with_capacity(len); ch];
    let mut block = vec![vec![0.0f32; OFFLINE_BLOCK]; ch];
    let cell = bound.cell();
    let ok = cell
        .with(|p| {
            let latency = p.latency() as i64;
            let mut done = 0usize;
            while done < len {
                let n = OFFLINE_BLOCK.min(len - done);
                for b in block.iter_mut() {
                    b.iter_mut().for_each(|x| *x = 0.0);
                }
                let at = c.start.saturating_add(done as i64);
                p.set_transport(&soundcraft_mix::transport_at(s, at.saturating_add(latency), true, false));
                p.set_loop(None);
                p.process(&mut block, n);
                for (o, b) in out.iter_mut().zip(&block) {
                    o.extend_from_slice(b.get(..n).unwrap_or_default());
                }
                done += n;
            }
            out.iter().all(|c| c.iter().all(|x| x.is_finite()))
        })
        .unwrap_or(false);
    drop(bound);
    if ok { Ok(out) } else { Err("the plug-in rendered audio that is not a number".into()) }
}

fn count_notes(d: &mut Doc, source: u64) {
    if d.doc.source_content_available(source, kARAContentTypeNotes)
        && let Some(n) = d.doc.source_notes(source)
    {
        d.notes.insert(source, n.len());
    }
}
