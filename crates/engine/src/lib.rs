//! The SoundCraft command engine.
//!
//! Every user-visible action is a registered command ([`cmd::CommandSpec`]): menus, shortcuts, the
//! CLI, the JSON control channel and MCP all dispatch the same commands by id with JSON params.
//! The engine owns the [`Session`] behind an `Arc` (copy-on-write), so undo snapshots are cheap.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod amp_irs;
pub mod autosave;
pub mod band_print;
pub mod brand;
pub mod catalog;
pub mod catalog_more;
pub mod clip_group_file;
pub mod cmd;
pub mod dawproject;
pub mod demo;
pub mod edit;
pub mod file_access;
pub mod inspect;
pub mod io;
pub mod juke_kits;
pub mod lexicon;
pub mod lyric_text;
pub mod lyric_tools;
pub mod lyrics;
pub mod midi_cleanup;
pub mod midi_record;
#[cfg(not(target_arch = "wasm32"))]
pub mod ml;
pub mod module_record;
pub mod open_job;
pub mod pattern;
pub mod peak_cache;
pub mod render_job;
pub mod score;
pub mod scout_profiles;
pub mod scout_prompt;
pub mod spill;
pub mod split_files;
pub mod tempo_detect;
pub mod vocal;
pub mod vocal_studio;

use serde_json::{Value, json};
use soundcraft_model::{Clip, Session, TrackId, ZoomState};
use soundcraft_time::Samples;
use std::cell::Cell;
use std::sync::Arc;

pub use cmd::{COMMAND_ALIASES, CommandInfo, CommandSpec, canonical_id, command_specs, find_command};
pub use soundcraft_model as model;

/// Shuts the third-party plugin hosts down, running the module exit of every plugin binary they
/// loaded. Call it at the end of `main`, once every plugin instance (mixer, player) is gone: a
/// plugin that has been used can crash when its module exit only runs from an exit handler.
pub fn shutdown_plugin_hosts() {
    soundcraft_vst3_host::shutdown();
}

/// Where the CLAP and VST3 hosts keep their plugin scan caches (`<config>/PluginScan`: `clap.json`,
/// `vst3.json`), so that the plugin lists and the plugins' locations survive restarts and later
/// scans only look at new and changed bundles. `None` keeps them in memory only.
/// For command-line tools: a plugin missing from the scan cache (a session or render needs it) runs
/// one blocking incremental scan through the scanner helper first, reporting its progress
/// (`report(done, total)`). See `soundcraft_vst3_host::set_blocking_lookups`.
pub fn set_blocking_plugin_lookups(report: Option<fn(usize, usize)>) {
    soundcraft_clap_host::set_blocking_lookups(report);
    soundcraft_vst3_host::set_blocking_lookups(report);
}

pub fn set_plugin_scan_cache_dir(dir: Option<std::path::PathBuf>) {
    soundcraft_clap_host::set_scan_cache_dir(dir.clone());
    soundcraft_vst3_host::set_scan_cache_dir(dir);
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EngineError {
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("`{0}` is not available: {1}")]
    Disabled(String, String),
    #[error("bad parameters for `{0}`: {1}")]
    BadParams(String, String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Io(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("internal error in `{0}`: {1}")]
    Internal(String, String),
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// Requests for the transport/audio engine, drained by the host app each frame.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum TransportRequest {
    /// Stop the old session's transport without applying stop-time edits to the new document.
    ResetSession,
    Play,
    Stop,
    TogglePlay,
    /// Record; with `countoff` false the take starts at once even with Options › Count Off on.
    Record {
        countoff: bool,
    },
    Locate(Samples),
    PlaySelection,
    Pause,
    HalfSpeed,
    Scrub(Samples),
    AllNotesOff,
    /// `scout.capture` start: build a Scout rig for `strips` (off the audio thread) and arm it.
    /// With `start`/`end` (a loop capture) arm it and play the loop from `start` in one step
    /// (`Player::scout_loop`).
    ScoutStart {
        capture: u32,
        strips: Vec<TrackId>,
        start: Option<Samples>,
        end: Option<Samples>,
    },
    /// `scout.capture` stop: end the armed capture (it comes back to be stored with `scout.store`).
    ScoutStop,
}

/// Snapshot of the transport reported back by the host (playback position etc.).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize)]
pub struct TransportStatus {
    pub playing: bool,
    pub recording: bool,
    pub position: Samples,
}

/// Clipboard contents: per-track clip copies relative to the copied range start.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Clipboard {
    pub length: Samples,
    pub tracks: Vec<Vec<Clip>>,
    pub automation: Vec<Vec<soundcraft_model::AutomationLane>>,
    /// Copy Special › Clip Gain: per track, (offset from the copied range start, dB) breakpoints.
    pub clip_gain: Vec<Vec<(Samples, f32)>>,
    /// Copy Special › Clip Effects: clip-effect settings (`param` → value) of the first copied clip.
    pub clip_effects: Vec<(String, f64)>,
    /// Markers inside the copied range (starts relative to it), for Paste Special › Merge Markers.
    pub markers: Vec<soundcraft_model::MemoryLocation>,
    /// How far the copied range started after a grid line; Relative Grid pastes keep it.
    pub grid_offset: Samples,
    /// Copy Special › Automation: pasting it leaves the clips alone, in Shuffle mode too.
    pub automation_only: bool,
    /// `automation.copy`: breakpoints of one lane, timed from the first, and the lane they came
    /// from (`automation.paste` takes a lane of the same kind).
    pub auto_points: Vec<soundcraft_model::AutomationPoint>,
    pub auto_param: Option<soundcraft_model::AutoParam>,
}

impl Clipboard {
    /// An empty clipboard for material copied from `range`.
    pub fn of_range(s: &Session, range: soundcraft_time::Range) -> Clipboard {
        let line = s.edit.grid.line_at_or_before(range.start, s.sample_rate, &s.tempo, s.frame_rate);
        Clipboard { length: range.len(), grid_offset: range.start.saturating_sub(line), ..Clipboard::default() }
    }
}

#[derive(Debug, Clone)]
struct HistoryEntry {
    label: String,
    doc: Arc<Session>,
}

/// Engine state: the document, history, clipboard and transport plumbing.
pub struct Engine {
    doc: Arc<Session>,
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    pub history_limit: usize,
    pub clipboard: Clipboard,
    pub transport_requests: Vec<TransportRequest>,
    pub transport: TransportStatus,
    /// Command journal (id, params) for macro recording / agents.
    pub journal: Vec<(String, Value)>,
    /// Path of the session file, if saved.
    pub path: Option<String>,
    /// Revision counter; bumps on every document change but zoom/scroll (UI caches key on it).
    pub revision: u64,
    dirty: bool,
    /// The document as last opened or saved, for `is_dirty`.
    saved: Arc<Session>,
    /// `is_dirty` as of a revision.
    dirty_at: Cell<(u64, bool)>,
    /// Gesture key for undo coalescing (see `execute_merged`).
    merge_key: Option<String>,
    /// The key every merged gesture takes while held (see `hold_merge`).
    merge_hold: Option<String>,
    /// The redo stack a gesture's first step cleared, kept while that step is still the newest
    /// and brought back when the step is discarded (see `undo_discard`): a drag that ends where
    /// it began costs no Redo.
    merge_redo: Option<Vec<HistoryEntry>>,
    /// The undo label the running command asks for instead of its spec's (one command whose
    /// kinds read differently in Edit › Undo, as `vocal.step`'s Centre Note / Flatten Drift).
    pub(crate) step_label: Option<&'static str>,
    /// Messages for the UI status line / agents.
    pub messages: Vec<String>,
    /// Silent sidechains found when the session was last loaded, for the next command's result.
    sidechain_notes: Vec<String>,
    /// Counts sessions loaded (`note_loaded_sidechains`): a command that loads one reports what the
    /// loaded session holds, not a comparison with the session it replaced.
    loads: u64,
    /// The edit selection (range, tracks) before the last change, for Restore Last Selection.
    /// Kept out of the document so selecting never marks the session modified.
    pub last_selection: Option<(soundcraft_time::Range, Vec<TrackId>)>,
    /// The Scout capture started and not yet stopped or stored (`scout.capture`, `scout.store`).
    pub scout_capture: Option<u32>,
    /// Captures started by `scout.capture` and not yet stored: `scout.store` takes only these.
    pub scout_pending: Vec<u32>,
    /// Every Scout id handed out is below this. It never decreases (not on undo, not when the
    /// session is replaced), so a capture id is never reused: a rig of an undone or replaced
    /// capture can never match a later one.
    pub(crate) scout_floor: u32,
    /// The Band Library: the embedded one plus the user folder's players and targets (`None` =
    /// the embedded Library alone). Not part of the document.
    band_lib: Option<Arc<soundcraft_band::library::LibraryData>>,
    /// The user's Band folder (`<config dir>/Band`): `Players`, `Targets`, `Bands`, `FeelPresets.json`.
    band_folder: Option<std::path::PathBuf>,
    /// Renders a host made ahead of `track.freeze` / `track.commit` through its live plugin
    /// instances ([`render_job::RenderJob`]), for the next command to use instead of rendering:
    /// good for the revision they were made at, cleared by the command that runs.
    prepared: Vec<render_job::PreparedRender>,
    prepared_rev: u64,
    /// Pieces Edit › Split into Files copied ahead of the command ([`split_files::SplitJob`]),
    /// for the next command to use instead of copying; cleared by the command that runs.
    split_pieces: Vec<split_files::Piece>,
    /// Live automation writing: what is touched, what this pass wrote ([`cmd::auto_write`]).
    pub(crate) auto_write: cmd::auto_write::AutoWriter,
    /// Imported audio is copied into the project's `Audio Files` when the session is saved (the
    /// default); off, a file imported at the session rate stays where it is and the project
    /// refers to it (`setup.copy_imported_audio`; the app's Preferences set it).
    pub copy_imported_audio: bool,
    /// Where a session never saved keeps the audio it makes (`<root>/<session name>/Audio
    /// Files/…`, beside its takes): the app's autosave folder. `None`: such audio stays in memory
    /// until the first save ([`spill`]).
    pub unsaved_audio_root: Option<std::path::PathBuf>,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new(Session::default())
    }
}

impl Engine {
    pub fn new(mut session: Session) -> Self {
        install_panic_hook();
        crate::cmd::module_ceiling::prepare_states(&mut session);
        crate::midi_cleanup::refresh(&mut session);
        crate::vocal::refresh_fingerprints_in(&mut session);
        let doc = Arc::new(session);
        Engine {
            saved: Arc::clone(&doc),
            doc,
            undo: Vec::new(),
            redo: Vec::new(),
            history_limit: 200,
            clipboard: Clipboard::default(),
            transport_requests: Vec::new(),
            transport: TransportStatus::default(),
            journal: Vec::new(),
            path: None,
            revision: 1,
            dirty: false,
            dirty_at: Cell::new((0, false)),
            merge_key: None,
            merge_redo: None,
            merge_hold: None,
            step_label: None,
            messages: Vec::new(),
            sidechain_notes: Vec::new(),
            loads: 0,
            last_selection: None,
            scout_capture: None,
            scout_pending: Vec::new(),
            scout_floor: 0,
            band_lib: None,
            band_folder: None,
            prepared: Vec::new(),
            prepared_rev: 0,
            split_pieces: Vec::new(),
            auto_write: cmd::auto_write::AutoWriter::new(),
            copy_imported_audio: true,
            unsaved_audio_root: None,
        }
    }

    /// The Band Library the band plays from (the embedded one until a user folder is set).
    pub fn band_library(&self) -> Arc<soundcraft_band::library::LibraryData> {
        self.band_lib.clone().unwrap_or_else(soundcraft_band::library::embedded_library)
    }

    /// Sets the user's Band folder (`<config dir>/Band`) and loads the Library from it. `None`, and
    /// every folder on wasm (there is none there), means the embedded Library alone.
    pub fn set_band_user_folder(&mut self, dir: Option<std::path::PathBuf>) {
        self.band_folder = if cfg!(target_arch = "wasm32") { None } else { dir };
        self.reload_band_library();
    }

    /// The user's Band folder, if one is set.
    pub fn band_user_folder(&self) -> Option<&std::path::Path> {
        self.band_folder.as_deref()
    }

    /// Reads the Library again (the user folder's files may have changed); returns its errors.
    pub fn reload_band_library(&mut self) -> Vec<String> {
        use soundcraft_band::library::{LibraryOverlay, LibrarySource, load_library};
        self.band_lib = self.band_folder.as_deref().map(|d| load_library(&LibrarySource::with_user_folder(d), &LibraryOverlay::default()));
        self.band_library().errors.clone()
    }

    /// Sessions put in place so far (`replace_session`, crash recovery): tells work started in
    /// one session from the next.
    pub fn loads(&self) -> u64 {
        self.loads
    }

    /// Every Scout id handed out so far is below this (it never decreases).
    pub fn scout_floor(&self) -> u32 {
        self.scout_floor
    }

    pub fn session(&self) -> &Session {
        &self.doc
    }

    pub fn session_arc(&self) -> Arc<Session> {
        Arc::clone(&self.doc)
    }

    /// Mutable access (copy-on-write). Callers outside commands should prefer `execute`.
    pub fn session_mut(&mut self) -> &mut Session {
        self.revision = self.revision.wrapping_add(1);
        self.dirty = true;
        Arc::make_mut(&mut self.doc)
    }

    /// Keeps renders made ahead of the next `track.freeze` / `track.commit` (through a host's
    /// live instances) for that command: it takes them with [`Engine::take_prepared`] instead
    /// of rendering, as long as the session has not changed meanwhile.
    pub fn offer_renders(&mut self, renders: Vec<render_job::PreparedRender>) {
        self.prepared = renders;
        self.prepared_rev = self.revision;
    }

    /// Keeps the pieces a [`split_files::SplitJob`] copied for the next `edit.split_into_files`
    /// (each is found by its source and window, so a session changed meanwhile is no matter).
    pub fn offer_split_pieces(&mut self, pieces: Vec<split_files::Piece>) {
        self.split_pieces = pieces;
    }

    /// The pieces offered for `edit.split_into_files` (see [`Engine::offer_split_pieces`]).
    pub(crate) fn take_split_pieces(&mut self) -> Vec<split_files::Piece> {
        std::mem::take(&mut self.split_pieces)
    }

    /// The render made ahead for `track` and `kind`, if any (see [`Engine::offer_renders`]).
    pub fn take_prepared(&mut self, track: TrackId, kind: &render_job::RenderKind) -> Option<Vec<Vec<f32>>> {
        let at = self.prepared.iter().position(|r| r.track == track && r.kind == *kind)?;
        Some(self.prepared.swap_remove(at).audio)
    }

    /// Makes `r` the loop without marking the session modified: the saved snapshot takes it too,
    /// as a selection change (which set it) never counts as an edit.
    /// The selection change that set it already bumped `revision`, so this does not.
    pub(crate) fn set_loop_quietly(&mut self, r: soundcraft_time::Range) {
        Arc::make_mut(&mut self.doc).edit.loop_range = Some(r);
        Arc::make_mut(&mut self.saved).edit.loop_range = Some(r);
    }

    /// Like `session_mut`, for `edit.zoom` only and without bumping `revision`.
    pub fn zoom_mut(&mut self) -> &mut ZoomState {
        self.dirty = true;
        &mut Arc::make_mut(&mut self.doc).edit.zoom
    }

    /// Replace the whole document (open / new); clears history.
    pub fn replace_session(&mut self, mut s: Session) {
        // Lanes saved with lowercased plugin parameter ids (see `restore_plugin_param_ids`).
        crate::cmd::restore_plugin_param_ids(&mut s);
        // Built-in modules' stored states (CleanQ's ceiling), made ready for the audio thread.
        crate::cmd::module_ceiling::prepare_states(&mut s);
        let thawed = crate::io::thaw_missing_renders(&mut s);
        // MIDI Cleanup previews are derived, not saved.
        crate::midi_cleanup::refresh(&mut s);
        // Vocal tuning fingerprints and caches are derived, not saved.
        crate::vocal::prune(&mut s);
        crate::vocal::refresh_fingerprints_in(&mut s);
        self.transport = TransportStatus { position: s.edit.selection.start.max(0), ..TransportStatus::default() };
        self.transport_requests.clear();
        self.transport_requests.push(TransportRequest::ResetSession);
        self.doc = Arc::new(s);
        self.saved = Arc::clone(&self.doc);
        self.undo.clear();
        self.redo.clear();
        self.merge_redo = None;
        self.last_selection = None;
        // The host stops the transport (`ResetSession`), which ends any armed capture.
        self.scout_capture = None;
        // A capture of the old session never lands in the new one.
        self.scout_pending.clear();
        self.revision = self.revision.wrapping_add(1);
        self.dirty = false;
        self.note_loaded_sidechains();
        for n in thawed {
            self.message(n);
        }
    }

    /// Reports the silent sidechains of the session now in place as just loaded (messages, and
    /// the next command's result). `replace_session` calls it; so does anything that puts a
    /// session in place another way (crash recovery).
    pub fn note_loaded_sidechains(&mut self) {
        self.loads = self.loads.wrapping_add(1);
        self.sidechain_notes = cmd::module::sidechain_report(None, &self.doc);
        for n in self.sidechain_notes.clone() {
            self.message(n);
        }
    }

    /// Makes every module asset (Amp IRs, reference audio) ready at the session's current sample
    /// rate where it is not yet (the rate changed); returns what could not be made, for the
    /// "missing media" report. Cheap when everything is ready.
    pub(crate) fn remake_assets_at_rate(&mut self) -> Vec<String> {
        let rate = self.doc.sample_rate.hz();
        let ready = self
            .doc
            .tracks
            .iter()
            .flat_map(|t| t.mixer.inserts.iter())
            .all(|i| i.assets.iter().all(|(k, r)| self.doc.assets.contains_for(k, r, rate)));
        if ready {
            return Vec::new();
        }
        let file = self.path.clone();
        cmd::module_asset::load_session_assets(Arc::make_mut(&mut self.doc), file.as_deref().map(std::path::Path::new))
    }

    /// Takes the silent sidechains reported when a session was last loaded and not yet shown.
    pub fn take_sidechain_notes(&mut self) -> Vec<String> {
        std::mem::take(&mut self.sidechain_notes)
    }

    /// Whether the document changed since it was opened or saved. Selecting, locating and zooming
    /// don't count, and undoing back to the saved state makes it clean again.
    pub fn is_dirty(&self) -> bool {
        if !self.dirty {
            return false;
        }
        let (rev, dirty) = self.dirty_at.get();
        if rev == self.revision {
            return dirty;
        }
        let dirty = !same_document(&self.saved, &self.doc);
        self.dirty_at.set((self.revision, dirty));
        dirty
    }
    /// Marks the document changed from what is on disk (a new session made from an import),
    /// so closing it asks to save.
    pub fn mark_unsaved(&mut self) {
        self.saved = Arc::new(Session::default());
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn mark_clean(&mut self) {
        self.dirty = false;
        self.saved = Arc::clone(&self.doc);
    }

    /// Puts each `new` in place of its `old` (the same audio, now played from its file) wherever
    /// the document, its saved snapshot or its undo history holds it, so the memory goes.
    pub(crate) fn swap_audio(&mut self, swaps: &[(Arc<soundcraft_model::SourceAudio>, Arc<soundcraft_model::SourceAudio>)]) {
        if swaps.is_empty() {
            return;
        }
        let fix = |doc: &mut Arc<Session>| {
            let hits: Vec<_> = doc
                .pool
                .ids()
                .filter_map(|id| {
                    let a = doc.pool.get(id)?;
                    swaps.iter().find(|(old, _)| Arc::ptr_eq(old, a)).map(|(_, new)| (id, Arc::clone(new)))
                })
                .collect();
            if !hits.is_empty() {
                let d = Arc::make_mut(doc);
                for (id, new) in hits {
                    d.pool.insert(id, new);
                }
            }
        };
        fix(&mut self.doc);
        fix(&mut self.saved);
        for h in self.undo.iter_mut().chain(self.redo.iter_mut()).chain(self.merge_redo.iter_mut().flatten()) {
            fix(&mut h.doc);
        }
        self.revision = self.revision.wrapping_add(1);
    }

    /// The document and every undo and redo step's document (what files the session may still use).
    pub fn history_sessions(&self) -> Vec<&Session> {
        std::iter::once(&*self.doc)
            .chain(std::iter::once(&*self.saved))
            .chain(self.undo.iter().chain(&self.redo).chain(self.merge_redo.iter().flatten()).map(|h| &*h.doc))
            .collect()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|h| h.label.as_str())
    }
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|h| h.label.as_str())
    }
    pub fn undo_history(&self) -> Vec<String> {
        self.undo.iter().map(|h| h.label.clone()).collect()
    }

    pub fn undo(&mut self) -> bool {
        self.merge_redo = None;
        let Some(h) = self.undo.pop() else { return false };
        let cur = std::mem::replace(&mut self.doc, h.doc);
        self.keep_edit_mode(&cur);
        self.keep_vocal_cache(&cur);
        self.refresh_vocal_fingerprints();
        self.redo.push(HistoryEntry { label: h.label, doc: cur });
        self.revision = self.revision.wrapping_add(1);
        self.dirty = true;
        true
    }

    /// Undoes the last step without offering it for redo: a compound edit that failed halfway
    /// puts the document back as it was, and a gesture (`execute_merged`) that ends where it began
    /// leaves no step. Only for directly after the caller's own step was just pushed: the redo
    /// stack that step cleared comes back only when the step is a gesture's (it is then as if the
    /// gesture never ran), and unlike [`Engine::undo`] it does not keep the current Edit mode.
    pub fn undo_discard(&mut self) -> bool {
        let Some(h) = self.undo.pop() else { return false };
        let cur = std::mem::replace(&mut self.doc, h.doc);
        self.keep_vocal_cache(&cur);
        self.refresh_vocal_fingerprints();
        self.revision = self.revision.wrapping_add(1);
        self.dirty = true;
        self.merge_key = None;
        if let Some(redo) = self.merge_redo.take() {
            self.redo = redo;
        }
        true
    }

    pub fn redo(&mut self) -> bool {
        self.merge_redo = None;
        let Some(h) = self.redo.pop() else { return false };
        let cur = std::mem::replace(&mut self.doc, h.doc);
        self.keep_edit_mode(&cur);
        self.keep_vocal_cache(&cur);
        self.refresh_vocal_fingerprints();
        self.undo.push(HistoryEntry { label: h.label, doc: cur });
        self.revision = self.revision.wrapping_add(1);
        self.dirty = true;
        true
    }

    /// Undo and Redo never change the Edit mode or its settings (Shuffle Lock above all).
    fn keep_edit_mode(&mut self, from: &Session) {
        use edit::{RELATIVE_GRID, SHUFFLE_LOCK, SNAP_TO_GRID};
        let flags = [SNAP_TO_GRID, SHUFFLE_LOCK, RELATIVE_GRID];
        let ed = &self.doc.edit;
        if ed.edit_mode != from.edit.edit_mode || flags.iter().any(|f| ed.flag(f) != from.edit.flag(f)) {
            let ed = &mut Arc::make_mut(&mut self.doc).edit;
            ed.edit_mode = from.edit.edit_mode;
            for f in flags {
                ed.set_flag(f, from.edit.flag(f));
            }
        }
    }

    /// Undo and Redo keep the derived vocal analyses and renders (`Session::vocal`): history
    /// entries are whole-session snapshots, and bringing an old one back must not throw away work
    /// done since (every vocal clip would be analysed again). Lookups are by inputs (analysis key,
    /// clip and render fingerprint), so entries the restored session doesn't use are just unused.
    fn keep_vocal_cache(&mut self, from: &Session) {
        if !self.doc.vocal.same(&from.vocal) {
            Arc::make_mut(&mut self.doc).vocal = from.vocal.clone();
        }
    }

    /// Brings every vocal tuning's render fingerprint up to date (`VocalTune::fingerprint`), so
    /// playback plays a cached render only when it was made from the tuning as it is now (never a
    /// newer render carried across Undo). No revision bump: derived data.
    pub(crate) fn refresh_vocal_fingerprints(&mut self) {
        crate::vocal::refresh_fingerprints(&mut self.doc);
    }

    /// Record an undo point manually (for interactive gestures committed by the UI).
    ///
    /// Stale MIDI Cleanup previews are refreshed first, so an edit made outside `execute` (a UI
    /// gesture, a background ML result such as Detect Chords) plays the cleaned notes of the
    /// chords and notes it left, and Undo/Redo bring back a step with its matching preview.
    pub fn push_undo(&mut self, label: &str, before: Arc<Session>) {
        if Arc::ptr_eq(&before, &self.doc) {
            return;
        }
        crate::pattern::refresh(&mut self.doc);
        self.refresh_cleanup();
        self.refresh_vocal_fingerprints();
        // Audio this step made goes to disk now, so neither the session nor its history holds it.
        spill::spill(self, label);
        self.undo.push(HistoryEntry { label: label.to_string(), doc: before });
        self.redo.clear();
        self.merge_redo = None;
        if self.undo.len() > self.history_limit {
            let excess = self.undo.len() - self.history_limit;
            self.undo.drain(..excess);
        }
    }

    /// Re-derives every MIDI Cleanup preview whose inputs changed; a hash scan first, so a session
    /// whose previews are current is not copied.
    fn refresh_cleanup(&mut self) {
        if crate::midi_cleanup::any_stale(&self.doc) {
            crate::midi_cleanup::refresh(Arc::make_mut(&mut self.doc));
        }
    }

    /// Whether command `id` must wait for the take being recorded: nothing may replace the
    /// session or quit under it.
    pub fn blocked_while_recording(&self, id: &str) -> bool {
        self.transport.recording && matches!(id, "session.new" | "session.open" | "session.close" | "session.revert" | "app.quit")
    }

    /// Run a command by id. Never panics: escaped panics become `EngineError::Internal` and the
    /// document is restored.
    pub fn execute(&mut self, id: &str, params: &Value) -> Result<Value> {
        let id = cmd::canonical_id(id);
        let spec = find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.to_string()))?;
        if self.blocked_while_recording(id) {
            return Err(EngineError::Disabled(id.to_string(), "stop recording first".into()));
        }
        if let Err(reason) = (spec.enabled)(self) {
            // A call that names its targets (tracks, clips, a range) does not need a selection;
            // the command itself validates them.
            let names_targets = ["track", "tracks", "clip", "clips", "sources", "start", "end", "at"].iter().any(|k| params.get(k).is_some());
            if !names_targets {
                return Err(EngineError::Disabled(id.to_string(), reason));
            }
        }
        if spec.view {
            return self.execute_view(spec, params);
        }
        // Renders made ahead for this command are good only for the session they were made from.
        if self.prepared_rev != self.revision {
            self.prepared.clear();
        }
        let before = Arc::clone(&self.doc);
        let rev = self.revision;
        let dirty = self.dirty;
        let loads = self.loads;
        self.step_label = None;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (spec.run)(self, params)));
        // Whatever the command did not use is of no use to the next.
        self.prepared.clear();
        self.split_pieces.clear();
        let result = match result {
            Ok(r) => r,
            Err(p) => {
                self.doc = before;
                self.revision = rev.wrapping_add(1);
                self.dirty = dirty;
                return Err(EngineError::Internal(id.to_string(), panic_message(&*p)));
            }
        };
        if !Arc::ptr_eq(&before, &self.doc) {
            // Only a change to the document ends a gesture, so Stop doesn't split the pass it ends.
            self.merge_key = None;
            self.merge_redo = None;
        }
        match result {
            Ok(mut v) => {
                // A frozen track's frozen parts are not edited: the command is undone.
                if !Arc::ptr_eq(&before, &self.doc)
                    && self.loads == loads
                    && let Some(why) = cmd::freeze::refused_edit(id, &before, &self.doc)
                {
                    self.doc = before;
                    self.revision = self.revision.wrapping_add(1);
                    self.dirty = dirty;
                    return Err(EngineError::Disabled(id.to_string(), why));
                }
                // No silent sidechain goes unreported: an edit that silences one (a routing change
                // closing a loop, a deactivated or deleted source) says so in its result.
                // Notes of a session loaded since the last command (by this one, or outside one,
                // as crash recovery does) come first; they were logged when it loaded.
                let mut notes = self.take_sidechain_notes();
                // A command that changed the session's sample rate in place: module assets (IRs,
                // references) are made per rate, so remake them; what can't be is reported.
                if self.loads == loads && before.sample_rate != self.doc.sample_rate {
                    for m in self.remake_assets_at_rate() {
                        let n = format!("missing media: {m}");
                        self.message(n.clone());
                        notes.push(n);
                    }
                }
                if !Arc::ptr_eq(&before, &self.doc) && self.loads == loads {
                    let fresh = cmd::module::sidechain_report(Some(&before), &self.doc);
                    for n in &fresh {
                        self.message(n.clone());
                    }
                    notes.extend(fresh.into_iter().filter(|n| !notes.contains(n)).collect::<Vec<_>>());
                }
                if !notes.is_empty()
                    && let Some(o) = v.as_object_mut()
                {
                    // `routing_warnings` is the general name (sidechains, links, media remade at a new
                    // rate); `sidechain_warnings` is kept for older clients.
                    o.insert("routing_warnings".into(), json!(notes));
                    o.insert("sidechain_warnings".into(), json!(notes));
                }
                // Pattern clips' notes follow their patterns, positions and the tempo map (first:
                // MIDI Cleanup derives from the notes).
                if !Arc::ptr_eq(&before, &self.doc) {
                    crate::pattern::refresh(&mut self.doc);
                }
                // MIDI Cleanup previews follow the notes, chords, key, tempo and settings they derive
                // from (`push_undo` does this for an undoable step).
                if !spec.undoable && !Arc::ptr_eq(&before, &self.doc) {
                    self.refresh_cleanup();
                }
                // Vocal tuning fingerprints follow the edits, key and clips they derive from.
                if !Arc::ptr_eq(&before, &self.doc) {
                    self.refresh_vocal_fingerprints();
                }
                cmd::remember_selection(self, &before);
                cmd::sync_loop(self, &before);
                let label = self.step_label.take().unwrap_or(spec.label);
                if spec.undoable && !Arc::ptr_eq(&before, &self.doc) {
                    self.push_undo(label, before);
                }
                if spec.journal {
                    self.journal.push((id.to_string(), params.clone()));
                }
                Ok(v)
            }
            Err(e) => {
                // Commands must not leave half-applied edits behind.
                if !Arc::ptr_eq(&before, &self.doc) {
                    self.doc = before;
                    self.revision = self.revision.wrapping_add(1);
                    self.dirty = dirty;
                }
                Err(e)
            }
        }
    }

    fn execute_view(&mut self, spec: &CommandSpec, params: &Value) -> Result<Value> {
        let zoom = self.doc.edit.zoom;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (spec.run)(self, params)))
            .unwrap_or_else(|p| Err(EngineError::Internal(spec.id.to_string(), panic_message(&*p))));
        if result.is_err() && self.doc.edit.zoom != zoom {
            Arc::make_mut(&mut self.doc).edit.zoom = zoom;
        }
        result
    }

    /// Execute as part of a continuous gesture (fader drag, knob turn, slider): consecutive calls
    /// with the same `key` collapse into a single undo step.
    pub fn execute_merged(&mut self, id: &str, params: &Value, key: &str) -> Result<Value> {
        let key = self.merge_hold.clone().unwrap_or_else(|| key.to_string());
        let continuing = self.merge_key.as_ref() == Some(&key);
        let before_len = self.undo.len();
        // The step pushed for a continuing tick is dropped again below, so it must not trim the
        // oldest entry first when the history is full.
        let limit = self.history_limit;
        if continuing && before_len > 0 {
            self.history_limit = limit.max(before_len.saturating_add(1));
        }
        // The gesture's first step clears Redo; kept aside in case the gesture is discarded
        // (`execute` drops it with the key each step, so a continuing step carries it across).
        let mut redo = if continuing { self.merge_redo.take() } else { Some(self.redo.clone()) };
        let r = self.execute(id, params);
        self.history_limit = limit;
        if continuing {
            self.merge_redo = redo.take();
        }
        let r = r?;
        if continuing && self.undo.len() == before_len + 1 && before_len > 0 {
            // Drop the step just pushed; the earlier one still holds the pre-gesture document.
            self.undo.pop();
        }
        if !continuing {
            self.merge_redo = redo.filter(|_| self.undo.len() == before_len + 1);
        }
        self.merge_key = Some(key);
        Ok(r)
    }

    /// Runs `id` as part of the undo step before it: its change joins that step, which still holds
    /// the document from before both (a preset loaded into the plugin just placed). With no step
    /// before it, it makes its own. Ends any gesture.
    pub fn execute_joined(&mut self, id: &str, params: &Value) -> Result<Value> {
        let before_len = self.undo.len();
        // The step pushed here is dropped again below, so it must not trim the oldest entry first
        // when the history is full.
        let limit = self.history_limit;
        if before_len > 0 {
            self.history_limit = limit.max(before_len.saturating_add(1));
        }
        let r = self.execute(id, params);
        self.history_limit = limit;
        self.merge_key = None;
        self.merge_redo = None;
        let r = r?;
        if before_len > 0 && self.undo.len() == before_len + 1 {
            self.undo.pop();
        }
        Ok(r)
    }

    /// While `key` is held, every `execute_merged` gesture takes it instead of its own, so they
    /// all join one undo step (an automation pass and whatever is dragged during it).
    pub fn hold_merge(&mut self, key: Option<&str>) {
        if self.merge_hold.as_deref() != key {
            self.merge_hold = key.map(str::to_string);
        }
    }

    /// End a gesture started with `execute_merged`.
    pub fn end_merge(&mut self) {
        self.merge_key = None;
    }

    /// Convenience: execute with `{}` params.
    pub fn run(&mut self, id: &str) -> Result<Value> {
        self.execute(id, &json!({}))
    }

    pub fn message(&mut self, m: impl Into<String>) {
        let m = m.into();
        log::info!("{m}");
        self.messages.push(m);
        if self.messages.len() > 100 {
            self.messages.remove(0);
        }
    }

    /// Tracks targeted by an edit: the selected tracks, or all tracks with playlists if none.
    pub fn target_tracks(&self) -> Vec<TrackId> {
        let s = self.session();
        if s.edit.selected_tracks.is_empty() {
            Vec::new()
        } else {
            s.tracks.iter().filter(|t| s.edit.selected_tracks.contains(&t.id)).map(|t| t.id).collect()
        }
    }
}

/// Equal apart from what changes without editing: the selection, the playhead and the zoom.
fn same_document(saved: &Session, doc: &Session) -> bool {
    let mut doc = doc.clone();
    let (from, to) = (&saved.edit, &mut doc.edit);
    to.selection = from.selection;
    to.timeline_selection = from.timeline_selection;
    to.selected_tracks.clone_from(&from.selected_tracks);
    to.selected_clips.clone_from(&from.selected_clips);
    to.playhead = from.playhead;
    to.zoom = from.zoom;
    *saved == doc
}

fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            log::error!("panic: {info}\n{}", std::backtrace::Backtrace::force_capture());
            prev(info);
        }));
    });
}

fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".into())
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    #[test]
    fn merged_gesture_is_one_undo_step() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, None);
        for db in [-1.0, -2.0, -3.0, -4.0] {
            e.execute_merged("mix.volume", &json!({"track": t.0, "db": db}), "fader").unwrap();
        }
        e.end_merge();
        assert_eq!(e.undo_history().len(), 1);
        e.undo();
        assert_eq!(e.session().track(t).unwrap().mixer.volume_db, 0.0);
    }

    #[test]
    fn a_joined_command_is_part_of_the_step_before_it() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, None);
        let pan = e.session().track(t).unwrap().mixer.pan.clone();
        e.execute("mix.volume", &json!({"track": t.0, "db": -1.0})).unwrap();
        e.execute("mix.volume", &json!({"track": t.0, "db": -2.0})).unwrap();
        e.execute_joined("mix.pan", &json!({"track": t.0, "pan": 0.5})).unwrap();
        assert_eq!(e.undo_history().len(), 2);
        e.undo();
        let m = &e.session().track(t).unwrap().mixer;
        assert_eq!((m.volume_db, &m.pan), (-1.0, &pan), "the pan went with the second volume");
        // A full history keeps its oldest step; a failed join leaves the history alone.
        e.history_limit = 2;
        e.execute("mix.volume", &json!({"track": t.0, "db": -3.0})).unwrap();
        e.execute_joined("mix.pan", &json!({"track": t.0, "pan": -0.5})).unwrap();
        assert_eq!(e.undo_history().len(), 2);
        assert!(e.execute_joined("mix.pan", &json!({"track": 999, "pan": 0.1})).is_err());
        assert_eq!(e.undo_history().len(), 2);
        e.undo();
        e.undo();
        assert_eq!(e.session().track(t).unwrap().mixer.volume_db, 0.0);
    }

    #[test]
    fn a_held_merge_joins_other_gestures_into_its_step() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, None);
        e.hold_merge(Some("pass"));
        for db in [-1.0, -2.0, -3.0] {
            e.execute_merged("mix.volume", &json!({"track": t.0, "db": db}), "fader").unwrap();
            e.execute_merged("mix.pan", &json!({"track": t.0, "pan": db / 4.0}), "pan").unwrap();
        }
        assert_eq!(e.undo_history().len(), 1);
        e.hold_merge(None);
        e.execute_merged("mix.volume", &json!({"track": t.0, "db": -6.0}), "fader").unwrap();
        assert_eq!(e.undo_history().len(), 2);
    }

    #[test]
    fn only_a_change_to_the_document_ends_a_gesture() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, None);
        e.execute_merged("mix.volume", &json!({"track": t.0, "db": -1.0}), "fader").unwrap();
        e.execute("transport.stop", &json!({})).unwrap();
        e.execute_merged("mix.volume", &json!({"track": t.0, "db": -2.0}), "fader").unwrap();
        assert_eq!(e.undo_history().len(), 1);
        e.execute("mix.mute", &json!({"track": t.0})).unwrap();
        e.execute_merged("mix.volume", &json!({"track": t.0, "db": -3.0}), "fader").unwrap();
        assert_eq!(e.undo_history().len(), 3);
    }
}

#[cfg(test)]
mod dirty_tests {
    use super::*;

    #[test]
    fn selecting_locating_and_zooming_leave_the_session_clean() {
        let mut e = crate::demo::demo_engine();
        for (id, p) in [
            ("edit.select", json!({"tracks": ["Kick"], "start": 0, "end": 48_000})),
            ("edit.select_none", json!({})),
            ("transport.locate", json!({"at": 96_000})),
            ("view.zoom_in", json!({})),
            ("view.scroll", json!({"to": 48_000})),
        ] {
            e.execute(id, &p).unwrap();
            assert!(!e.is_dirty(), "{id}");
        }
        e.execute("mix.mute", &json!({"track": "Kick"})).unwrap();
        assert!(e.is_dirty());
        e.undo();
        assert!(!e.is_dirty());
    }
}
