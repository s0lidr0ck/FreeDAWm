//! Modal dialogs opened by menu-style invocations.

use crate::SoundApp;
use crate::theme::{Tokens, bold};
use egui::{Align2, Stroke, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_engine::Engine;
use soundcraft_model::{ClipId, GroupId, Session, SourceId, TrackId};
use soundcraft_time::Samples;

/// The Bounce Mix dialog's Range choice (the sticky loop spec, rule 7): the loop, the stretch
/// between two markers (`to` None = the end of the session), or the whole session.
#[derive(Debug, Clone, PartialEq)]
pub enum BounceRange {
    Loop,
    Markers { from: soundcraft_model::MarkerId, to: Option<soundcraft_model::MarkerId> },
    Whole,
}

impl BounceRange {
    /// The default: the loop when there is one, else the whole session.
    pub fn default_for(s: &Session) -> BounceRange {
        if s.edit.loop_range.is_some_and(|r| !r.is_empty()) { BounceRange::Loop } else { BounceRange::Whole }
    }

    /// Markers of every marker ruler in timeline order, for the Between markers pickers.
    pub fn markers(s: &Session) -> Vec<&soundcraft_model::MemoryLocation> {
        let mut v: Vec<_> = s.markers.iter().filter(|m| m.track.is_none()).collect();
        v.sort_by_key(|m| (m.start, m.number));
        v
    }

    /// The `start`/`end` to bounce, or none for a choice that cannot be made (no loop, no markers).
    pub fn range(&self, s: &Session) -> Option<soundcraft_time::Range> {
        let end_of_session = s.content_end().max(1);
        match self {
            BounceRange::Loop => s.edit.loop_range.filter(|r| !r.is_empty()),
            BounceRange::Whole => Some(soundcraft_time::Range::new(0, end_of_session)),
            BounceRange::Markers { from, to } => {
                let at = |id: soundcraft_model::MarkerId| s.markers.iter().find(|m| m.id == id).map(|m| m.start);
                let a = at(*from)?;
                let b = match to {
                    Some(id) => at(*id)?,
                    None => end_of_session.max(a),
                };
                Some(soundcraft_time::Range::new(a, b.max(a)))
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum Dialog {
    NewTracks {
        count: u32,
        format: String,
        kind: String,
        timebase: String,
        name: String,
    },
    /// File › New… and File › New from Template…: a name, a rate and what to start from.
    NewSession(NewSession),
    /// Save Track Preset… and Save Plug-in Chain… (`chain`): a name for the preset, saved in the
    /// Library's track presets or chains folder.
    SaveTrackPreset {
        track: TrackId,
        name: String,
        chain: bool,
    },
    /// Load Track Preset… and Load Plug-in Chain… (`chain`): the Library's presets (newest first)
    /// or any preset file.
    LoadTrackPreset {
        tracks: Vec<u64>,
        chain: bool,
        path: String,
        /// The presets folder as it was when the dialog opened (read once, not every frame).
        presets: Vec<crate::user_files::UserFile>,
    },

    Bounce {
        path: String,
        format: String,
        bit_depth: String,
        /// kbps, for MP3 and Opus.
        bitrate: u32,
        normalize: bool,
        range: BounceRange,
    },
    RenameTrack {
        id: TrackId,
        name: String,
    },
    RenameClip {
        id: ClipId,
        name: String,
    },
    RenameMarker {
        id: soundcraft_model::MarkerId,
        name: String,
    },
    /// A breakpoint's Set Value…: its value typed in the lane's unit.
    PointValue {
        track: TrackId,
        param: soundcraft_model::AutoParam,
        at: soundcraft_time::Samples,
        text: String,
    },
    ClearClips {
        clips: Vec<ClipId>,
        sources: Vec<SourceId>,
        message: String,
    },
    PathPrompt {
        cmd: String,
        title: String,
        path: String,
        key: String,
    },
    Number {
        cmd: String,
        title: String,
        key: String,
        value: f64,
        suffix: String,
    },
    TempoChange {
        at: Samples,
        bpm: f64,
    }, // from double-clicking the tempo ruler
    /// The Key ruler's key picker: 12 roots × Major / Minor. `edit` is the key change it edits
    /// (at `at`); otherwise OK adds one at `at`.
    KeyPicker {
        at: Samples,
        edit: bool,
        root: u8,
        minor: bool,
    },
    /// The Chords ruler's chord picker (add or edit a chord).
    ChordPicker(crate::band::chord_picker::ChordPicker),
    Spot(crate::spot::Spot),
    Fades {
        shape: String,
    },
    /// Event › More MIDI › Change Velocity…: the selected notes in the MIDI Editor, else the
    /// selected MIDI clips.
    ChangeVelocity(VelocityDialog),
    StripSilence {
        threshold: f64,
        min_ms: f64,
        pre_ms: f64,
        post_ms: f64,
    },
    // `id: None` creates (Track › Group…); `id: Some` modifies that group (Groups list › Modify Group…).
    Group {
        id: Option<GroupId>,
        name: String,
        edit: bool,
        mix: bool,
        members: Vec<(u64, String, bool)>,
    },
    Session {
        frame_rate: String,
        bit_depth: String,
    },
    ScoreSetup {
        title: String,
        composer: String,
        bars_per_system: u32,
        show_track_names: bool,
    },
    /// Options › Pre/Post-Roll…: on/off and both amounts, in seconds.
    PrePostRoll {
        on: bool,
        pre: f64,
        post: f64,
    },
    SaveChanges {
        then: (String, Value),
    },
    /// A yes/no question before a command that removes things (Clear All Markers…), in the app
    /// (never an OS dialog). The command is one undo step.
    Confirm {
        title: String,
        message: String,
        ok: String,
        cmd: String,
        params: Value,
    },
}

/// What a new session starts from.
#[derive(Debug, Clone, PartialEq)]
pub enum Start {
    Blank,
    Demo,
    /// A template file (a saved template, or one browsed to).
    Template(std::path::PathBuf),
}

/// The New dialog's state.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub name: String,
    pub sample_rate: u32,
    pub start: Start,
    /// The user's templates, newest first (read when the dialog opened).
    pub templates: Vec<crate::user_files::UserFile>,
    /// The chosen template's details line, worked out once per choice.
    pub details: Option<(std::path::PathBuf, String)>,
    /// Browse…: a template file from anywhere, typed (or picked) here.
    pub browse: Option<String>,
}

impl NewSession {
    /// The dialog as File › New… opens it (Blank chosen), or as New from Template… does (the
    /// newest template chosen, Up and Down walking the list).
    pub fn new(dirs: &crate::user_files::UserDirs, from_template: bool) -> NewSession {
        let templates = crate::user_files::templates(dirs);
        let start = match templates.first() {
            Some(t) if from_template => Start::Template(t.path.clone()),
            _ => Start::Blank,
        };
        NewSession { name: "Untitled".into(), sample_rate: 48_000, start, templates, details: None, browse: None }
    }

    /// `session.new`'s parameters for the dialog as it stands.
    pub fn params(&self) -> Value {
        match &self.start {
            Start::Blank => json!({"name": self.name, "sample_rate": self.sample_rate, "template": "blank"}),
            Start::Demo => json!({"name": self.name, "sample_rate": self.sample_rate, "template": "demo"}),
            Start::Template(p) => json!({"name": self.name, "template": p.to_string_lossy()}),
        }
    }
}

impl Dialog {
    /// Load Track Preset… for `tracks`, listing the presets folder as it is now.
    pub fn load_track_preset(dirs: &crate::user_files::UserDirs, tracks: Vec<u64>) -> Dialog {
        Dialog::LoadTrackPreset { tracks, chain: false, path: String::new(), presets: crate::user_files::track_presets(dirs) }
    }

    pub fn load_chain(dirs: &crate::user_files::UserDirs, tracks: Vec<u64>) -> Dialog {
        Dialog::LoadTrackPreset { tracks, chain: true, path: String::new(), presets: crate::user_files::chains(dirs) }
    }
}

#[derive(Default)]
pub struct Dialogs {
    pub open: Option<Dialog>,
    /// The Library's folders (set by the desktop app).
    pub dirs: crate::user_files::UserDirs,
    /// Runs once the session has been saved from `SaveChanges`.
    after_save: Option<(String, Value)>,
    /// A session the browser read while the open one had changes, waiting for `SaveChanges`.
    pending_session: Option<(String, Vec<u8>)>,
}

impl Dialogs {
    pub fn open_name(&self) -> Option<&'static str> {
        self.open.as_ref().map(|d| match d {
            Dialog::NewTracks { .. } => "new_tracks",
            Dialog::NewSession(_) => "new_session",
            Dialog::SaveTrackPreset { .. } => "save_track_preset",
            Dialog::LoadTrackPreset { .. } => "load_track_preset",
            Dialog::Bounce { .. } => "bounce",
            Dialog::RenameTrack { .. } => "rename_track",
            Dialog::RenameClip { .. } => "rename_clip",
            Dialog::RenameMarker { .. } => "rename_marker",
            Dialog::PointValue { .. } => "point_value",
            Dialog::ClearClips { .. } => "clear_clips",
            Dialog::PathPrompt { .. } => "path",
            Dialog::Number { .. } => "number",
            Dialog::TempoChange { .. } => "tempo_change",
            Dialog::KeyPicker { .. } => "key_picker",
            Dialog::ChordPicker(_) => "chord_picker",
            Dialog::Spot(_) => "spot",
            Dialog::Fades { .. } => "fades",
            Dialog::StripSilence { .. } => "strip_silence",
            Dialog::ChangeVelocity(_) => "change_velocity",
            Dialog::Group { id: None, .. } => "group",
            Dialog::Group { id: Some(_), .. } => "modify_group",
            Dialog::Session { .. } => "session",
            Dialog::ScoreSetup { .. } => "score_setup",
            Dialog::PrePostRoll { .. } => "pre_post_roll",
            Dialog::SaveChanges { .. } => "save_changes",
            Dialog::Confirm { .. } => "confirm",
        })
    }

    pub fn open_rename_track(&mut self, id: TrackId, name: &str) {
        self.open = Some(Dialog::RenameTrack { id, name: name.to_string() });
    }

    pub fn open_rename_clip(&mut self, id: ClipId, name: &str) {
        self.open = Some(Dialog::RenameClip { id, name: name.to_string() });
    }

    pub fn open_rename_marker(&mut self, id: soundcraft_model::MarkerId, name: &str) {
        self.open = Some(Dialog::RenameMarker { id, name: name.to_string() });
    }

    /// Create Group, with `members` ticked (the strip's group box, the Groups list's +).
    pub fn open_new_group(&mut self, e: &Engine, members: &[TrackId]) {
        let s = e.session();
        let mut n = s.groups.len() + 1;
        while s.groups.iter().any(|g| g.name == format!("Group {n}")) {
            n += 1;
        }
        let members = s.tracks.iter().map(|t| (t.id.0, t.name.clone(), members.contains(&t.id))).collect();
        self.open = Some(Dialog::Group { id: None, name: format!("Group {n}"), edit: true, mix: true, members });
    }

    /// Groups list › Modify Group…: the creation dialog, pre-filled from an existing group. False
    /// when the group is gone (deleted between the click and the menu).
    pub fn open_modify_group(&mut self, e: &Engine, id: GroupId) -> bool {
        let s = e.session();
        let Some(g) = s.group(id) else { return false };
        let members = s.tracks.iter().map(|t| (t.id.0, t.name.clone(), g.members.contains(&t.id))).collect();
        self.open = Some(Dialog::Group { id: Some(id), name: g.name.clone(), edit: g.edit, mix: g.mix, members });
        true
    }

    /// Options › Pre/Post-Roll…, filled in from the session.
    pub fn open_pre_post_roll(&mut self, e: &Engine) {
        let ed = &e.session().edit;
        let sr = e.session().sample_rate;
        self.open = Some(Dialog::PrePostRoll { on: ed.pre_post_roll, pre: sr.seconds(ed.pre_roll), post: sr.seconds(ed.post_roll) });
    }

    /// Clip List › Clear: confirm before clips and files leave the session. False when there is nothing to clear.
    pub fn open_clear(&mut self, e: &Engine, clips: Vec<ClipId>, sources: Vec<SourceId>) -> bool {
        let Some(message) = clear_message(e.session(), &clips, &sources) else { return false };
        self.open = Some(Dialog::ClearClips { clips, sources, message });
        true
    }

    /// Open the dialog for a command; false when the command has no dialog.
    pub fn open_for_command(&mut self, e: &Engine, id: &str) -> bool {
        self.after_save = None;
        let home = default_dir();
        let name = e.session().name.clone();
        self.open = Some(match id {
            "clip.clear" => {
                let ed = &e.session().edit;
                return self.open_clear(e, ed.selected_clips.clone(), ed.selected_sources.clone());
            }
            "track.new" => {
                Dialog::NewTracks { count: 1, format: "Mono".into(), kind: "audio".into(), timebase: "samples".into(), name: "Audio".into() }
            }
            "session.new" => Dialog::NewSession(NewSession::new(&self.dirs, false)),
            "ui.new_from_template" => Dialog::NewSession(NewSession::new(&self.dirs, true)),
            "session.save_template" if self.dirs.templates.is_some() => {
                let dir = self.dirs.templates.as_deref().map(|d| d.display().to_string()).unwrap_or_default();
                let file = crate::user_files::file_name(&name);
                Dialog::PathPrompt {
                    cmd: id.into(),
                    title: "Save As Template".into(),
                    path: format!("{dir}/{file}.{}", soundcraft_model::SESSION_EXTENSION),
                    key: "path".into(),
                }
            }
            "track.save_preset" if self.dirs.track_presets.is_some() => {
                let Some(t) = e.target_tracks().first().and_then(|id| e.session().track(*id)) else { return false };
                Dialog::SaveTrackPreset { track: t.id, name: t.name.clone(), chain: false }
            }
            "track.save_chain" if self.dirs.chains.is_some() => {
                let Some(t) = e.target_tracks().first().and_then(|id| e.session().track(*id)) else { return false };
                Dialog::SaveTrackPreset { track: t.id, name: format!("{} chain", t.name), chain: true }
            }
            "track.load_chain" => {
                let tracks: Vec<u64> = e.target_tracks().iter().map(|t| t.0).collect();
                if tracks.is_empty() {
                    return false;
                }
                Dialog::load_chain(&self.dirs, tracks)
            }
            "track.load_preset" => {
                let tracks: Vec<u64> = e.target_tracks().iter().map(|t| t.0).collect();
                if tracks.is_empty() {
                    return false;
                }
                Dialog::load_track_preset(&self.dirs, tracks)
            }
            "file.bounce_mix" => {
                let dir = e
                    .path
                    .as_deref()
                    .and_then(|p| std::path::Path::new(p).parent())
                    .filter(|d| !d.as_os_str().is_empty())
                    .map_or(home.clone(), |d| format!("{}/Bounced Files", d.display()));
                Dialog::Bounce {
                    path: format!("{dir}/{name} Bounce.wav"),
                    format: "wav".into(),
                    bit_depth: "24".into(),
                    bitrate: 0,
                    normalize: false,
                    range: BounceRange::default_for(e.session()),
                }
            }
            "file.import_audio" => Dialog::PathPrompt { cmd: id.into(), title: "Import Audio".into(), path: home, key: "path".into() },
            "file.import_midi" => Dialog::PathPrompt { cmd: id.into(), title: "Import MIDI".into(), path: home, key: "path".into() },
            "session.open" => Dialog::PathPrompt { cmd: id.into(), title: "Open Session".into(), path: home, key: "path".into() },
            "session.save" if e.path.is_some() => return false,
            "session.save" | "session.save_as" | "session.save_copy" => Dialog::PathPrompt {
                cmd: if id == "session.save_copy" { id } else { "session.save_as" }.into(),
                title: "Save Session As".into(),
                // The browser has no folders: the name becomes the bundle file's name.
                path: if home.is_empty() {
                    format!("{name}.{}", soundcraft_engine::io::bundle_extension_for(e.path.as_deref().unwrap_or_default()))
                } else {
                    format!("{home}/{name}/{name}.{}", soundcraft_model::SESSION_EXTENSION)
                },
                key: "path".into(),
            },
            "edit.repeat" => Dialog::Number { cmd: id.into(), title: "Repeat".into(), key: "count".into(), value: 2.0, suffix: "times".into() },
            "edit.shift" => {
                Dialog::Number { cmd: id.into(), title: "Shift (seconds)".into(), key: "by_seconds".into(), value: 1.0, suffix: "s".into() }
            }
            "event.transpose" => {
                Dialog::Number { cmd: id.into(), title: "Transpose".into(), key: "semitones".into(), value: 12.0, suffix: "semitones".into() }
            }
            "event.quantize" => {
                Dialog::Number { cmd: id.into(), title: "Quantize strength".into(), key: "strength".into(), value: 100.0, suffix: "%".into() }
            }
            "edit.fades_create" => Dialog::Fades { shape: "equal power".into() },
            "event.change_velocity" => Dialog::ChangeVelocity(VelocityDialog::default()),
            "edit.strip_silence" => Dialog::StripSilence { threshold: -48.0, min_ms: 50.0, pre_ms: 5.0, post_ms: 20.0 },
            "track.group" => {
                self.open_new_group(e, &e.session().edit.selected_tracks.clone());
                return true;
            }
            "setup.session" => Dialog::Session { frame_rate: e.session().frame_rate.label().into(), bit_depth: "24".into() },
            "file.score_setup" => {
                let s = soundcraft_engine::score::ScoreSetup::from_session(e.session());
                Dialog::ScoreSetup { title: s.title, composer: s.composer, bars_per_system: s.bars_per_system, show_track_names: s.show_track_names }
            }
            "track.rename" => {
                let Some(t) = e.target_tracks().first().and_then(|id| e.session().track(*id)) else { return false };
                Dialog::RenameTrack { id: t.id, name: t.name.clone() }
            }
            "clip.clean_mix" => {
                let s = e.session();
                let alt =
                    soundcraft_engine::cmd::clip_ids_param(e, &json!({})).into_iter().find_map(|id| s.find_clip(id).and_then(|(_, c)| c.clean_alt()));
                let Some(alt) = alt else { return false };
                Dialog::Number {
                    cmd: id.into(),
                    title: "Clean Vocal Amount".into(),
                    key: "mix".into(),
                    value: (f64::from(alt.mix) * 100.0).round(),
                    suffix: "% cleaned".into(),
                }
            }
            "clip.rename" => {
                let target = soundcraft_engine::cmd::clip_ids_param(e, &json!({})).first().and_then(|id| e.session().find_clip(*id));
                let Some((_, clip)) = target else { return false };
                Dialog::RenameClip { id: clip.id, name: clip.name.clone() }
            }
            _ => match path_dialog(id, &home, &name) {
                Some(d) => d,
                None => return false,
            },
        });
        true
    }
}

/// When `id` would close a session with unsaved changes, ask first; true when the question is
/// open instead. A take in progress is stopped first, so it counts as a change.
pub fn ask_to_save(app: &mut SoundApp, id: &str, params: &Value) -> bool {
    if !matches!(id, "session.new" | "session.open" | "session.close" | "session.revert" | "file.import_dawproject" | "app.quit" | OPEN_PENDING)
        || soundcraft_engine::find_command(id).is_some_and(|c| (c.enabled)(&app.engine).is_err())
    {
        return false;
    }
    if app.engine.transport.recording {
        app.stop_play();
    }
    if !app.engine.is_dirty() {
        return false;
    }
    app.dialogs.open = Some(Dialog::SaveChanges { then: (id.into(), params.clone()) });
    true
}

/// Runs `id`, unless it asks to save first (`ask_to_save`).
pub fn run_or_ask(app: &mut SoundApp, id: &str, params: Value) -> Result<Value, String> {
    if ask_to_save(app, id, &params) {
        return Ok(Value::Null);
    }
    app.run(id, params)
}

/// What `SaveChanges` carries on with to open the session the browser read.
const OPEN_PENDING: &str = "ui.open_pending_session";

/// Opens a session the browser read as bytes (picked or dropped), asking first when the open one
/// has changes. The read finishes later than the pick, so the question is asked only now.
pub fn open_session_bytes(app: &mut SoundApp, name: String, bytes: Vec<u8>) {
    if crate::bounce::refuses_session_switch(app) {
        return;
    }
    app.dialogs.pending_session = Some((name, bytes));
    if !ask_to_save(app, OPEN_PENDING, &json!({})) {
        open_pending_session(app);
    }
}

fn open_pending_session(app: &mut SoundApp) {
    let Some((name, bytes)) = app.dialogs.pending_session.take() else { return };
    if crate::bounce::refuses_session_switch(app) {
        return;
    }
    let _ = app.run("transport.stop", json!({}));
    app.ui.status = match soundcraft_engine::io::open_session_bytes(&mut app.engine, &name, &bytes) {
        Ok(missing) if missing.is_empty() => format!("Opened {name}"),
        Ok(missing) => format!("Opened {name} ({} missing media)", missing.len()),
        Err(e) => e.to_string(),
    };
}

/// A path prompt for any command whose parameters start with a required `path` or `dir` (plus
/// Print Score and Session Info as Text, whose path is optional: without one they only return
/// what they would write), with a default file name from the command.
pub fn takes_path(id: &str) -> bool {
    path_key(id).is_some()
}

fn path_key(id: &str) -> Option<&'static str> {
    let spec = soundcraft_engine::command_specs().iter().find(|c| c.id == id)?;
    if spec.params.starts_with("{path?") && !matches!(id, "file.print_score" | "file.export_session_text") {
        None
    } else if spec.params.starts_with("{path") {
        Some("path")
    } else if spec.params.starts_with("{dir") {
        Some("dir")
    } else {
        None
    }
}

fn path_dialog(id: &str, home: &str, name: &str) -> Option<Dialog> {
    let spec = soundcraft_engine::command_specs().iter().find(|c| c.id == id)?;
    let key = path_key(id)?;
    let importing = id.contains("import") || id.contains("open");
    let ext = match id {
        "file.export_midi" => ".mid",
        "file.export_lyrics" => ".lrc",
        "file.export_lyrics_text" => ".txt",
        "file.export_sibelius" => ".musicxml",
        "file.print_score" => ".svg",
        "file.export_clip_groups" => ".scgrp",
        "file.export_session_text" => ".txt",
        "file.export_dawproject" => ".dawproject",
        _ if id.contains("session") => ".fdawm",
        _ => "",
    };
    let path = if importing || key == "dir" { format!("{home}/") } else { format!("{home}/{name}{ext}") };
    let title = format!("{} — {}", spec.menu.last().copied().unwrap_or("File"), spec.label.trim_end_matches("..."));
    Some(Dialog::PathPrompt { cmd: id.into(), title, path, key: key.into() })
}

fn clear_message(s: &Session, clips: &[ClipId], sources: &[SourceId]) -> Option<String> {
    let listed: std::collections::BTreeSet<ClipId> = clips.iter().copied().collect();
    let names: Vec<&str> = s.tracks.iter().flat_map(|t| t.clips()).filter(|c| listed.contains(&c.id)).map(|c| c.name.as_str()).collect();
    let files: Vec<_> = sources.iter().filter_map(|id| s.source(*id)).collect();
    let count = |n: usize, what: &str| if n == 1 { format!("1 {what}") } else { format!("{n} {what}s") };
    let what = match (names.as_slice(), files.as_slice()) {
        ([], []) => return None,
        ([name], []) => format!("the clip \"{name}\""),
        ([], [f]) => format!("the file \"{}\"", f.name),
        (c, []) => count(c.len(), "clip"),
        ([], f) => count(f.len(), "file"),
        (c, f) => format!("{} and {}", count(c.len(), "clip"), count(f.len(), "file")),
    };
    let mut text = format!("Remove {what} from the session?");
    let used: Vec<(&str, usize)> =
        files.iter().map(|f| (f.name.as_str(), s.clips_using(f.id).filter(|c| !listed.contains(&c.id)).count())).filter(|(_, n)| *n > 0).collect();
    let n: usize = used.iter().map(|u| u.1).sum();
    let users = if n == 1 {
        "a clip on the timeline, which is removed too.".to_string()
    } else {
        format!("{n} clips on the timeline, which are removed too.")
    };
    match used.as_slice() {
        [] => {}
        [_] if files.len() == 1 => text += &format!(" It is used by {users}"),
        u if u.len() == files.len() => text += &format!(" They are used by {users}"),
        [(name, _)] => text += &format!(" \"{name}\" is used by {users}"),
        many => text += &format!(" {} of the files are used by {users}", many.len()),
    }
    if !files.is_empty() && files.iter().all(|f| !f.unsaved) {
        text += if files.len() == 1 { " The file stays on disk." } else { " The files stay on disk." };
    }
    Some(text)
}

fn default_dir() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| ".".into())
    }
    #[cfg(target_arch = "wasm32")]
    {
        String::new()
    }
}

/// The main window's dialogs: Process, bounce and ML progress, and the open dialog when the
/// main window opened it.
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    audiosuite_window(app, ctx);
    crate::open_progress::show(app, ctx);
    crate::dawproject_report::show(app, ctx);
    crate::bounce::show(app, ctx);
    crate::render::show(app, ctx);
    crate::split_progress::show(app, ctx);
    #[cfg(not(target_arch = "wasm32"))]
    crate::ml_ui::show(app, ctx);
    crate::tempo_ui::show(app, ctx);
    show_open(app, ctx);
}

/// The open dialog, if any, when it belongs to `ctx`'s viewport: a dialog opens over the window
/// that started it (the separate Mix window's rename, New Tracks…), centred on it.
pub fn show_open(app: &mut SoundApp, ctx: &egui::Context) {
    // A new dialog selects its name field's text once; the same dialog moving to the other window
    // (Mix docked or undocked) keeps what was typed.
    let name = app.dialogs.open_name();
    let shown = egui::Id::new("dialog-shown");
    if ctx.data(|d| d.get_temp::<Option<&'static str>>(shown)).flatten() != name {
        ctx.data_mut(|d| {
            d.insert_temp(shown, name);
            d.remove_temp::<bool>(egui::Id::new(NAME_SELECTED));
        });
    }
    if !crate::window_sync::draws_here(app.window_sync.dialog_home, ctx) {
        return;
    }
    let Some(mut d) = app.dialogs.open.take() else { return };
    let mut keep = true;
    // The Chords ruler's picker goes on at this tick after adding a chord played in.
    let mut chord_next: Option<i64> = None;
    let mut action: Option<(String, Value)> = None;
    let mut save_first: Option<(String, Value)> = None;
    let confirmed = matches!(d, Dialog::SaveChanges { .. });
    let title = match &d {
        Dialog::NewTracks { .. } => "New Tracks",
        Dialog::NewSession(_) => "New Session",
        Dialog::SaveTrackPreset { chain: false, .. } => "Save Track Preset",
        Dialog::LoadTrackPreset { chain: false, .. } => "Load Track Preset",
        Dialog::SaveTrackPreset { chain: true, .. } => "Save Plug-in Chain",
        Dialog::LoadTrackPreset { chain: true, .. } => "Load Plug-in Chain",
        Dialog::Bounce { .. } => "Bounce Mix",
        Dialog::RenameTrack { .. } => "Rename Track",
        Dialog::RenameClip { .. } => "Rename Clip",
        Dialog::RenameMarker { .. } => "Rename Marker",
        Dialog::PointValue { .. } => "Set Point Value",
        Dialog::ClearClips { .. } => "Clear",
        Dialog::PathPrompt { title, .. } | Dialog::Number { title, .. } => title.as_str(),
        Dialog::TempoChange { .. } => "Tempo Change (BPM)",
        Dialog::KeyPicker { edit: true, .. } => "Edit Key",
        Dialog::KeyPicker { .. } => "Key Change",
        Dialog::ChordPicker(p) if p.chord.is_some() => "Edit Chord",
        Dialog::ChordPicker(_) => "Add Chord",
        Dialog::Spot(_) => "Spot",
        Dialog::Fades { .. } => "Fades",
        Dialog::StripSilence { .. } => "Strip Silence",
        Dialog::ChangeVelocity(_) => "Change Velocity",
        Dialog::Group { id: None, .. } => "Create Group",
        Dialog::Group { id: Some(_), .. } => "Modify Group",
        Dialog::Session { .. } => "Session Setup",
        Dialog::ScoreSetup { .. } => "Score Setup",
        Dialog::PrePostRoll { .. } => "Pre/Post-Roll",
        Dialog::SaveChanges { then } if then.0 == "session.revert" => "Revert to Saved",
        Dialog::SaveChanges { .. } => "Unsaved Changes",
        Dialog::Confirm { title, .. } => title.as_str(),
    }
    .to_string();
    let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
    let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    egui::Window::new(egui::RichText::new(&title).font(bold(13.0)))
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, -80.0))
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            match &mut d {
                Dialog::NewTracks { count, format, kind, timebase, name } => {
                    ui.horizontal(|ui| {
                        ui.label("Create");
                        ui.add(egui::DragValue::new(count).range(1..=128));
                        ui.label("new");
                        egui::ComboBox::from_id_salt("nt_fmt").selected_text(format.as_str()).show_ui(ui, |ui| {
                            for f in soundcraft_model::ChannelFormat::ALL {
                                ui.selectable_value(format, f.label().to_string(), f.label());
                            }
                        });
                        egui::ComboBox::from_id_salt("nt_kind")
                            .selected_text(soundcraft_model::TrackKind::from_id(kind).map_or("Audio Track", |k| k.label()))
                            .show_ui(ui, |ui| {
                                for k in soundcraft_model::TrackKind::ALL {
                                    if ui.selectable_label(kind == k.id(), k.label()).clicked() {
                                        *kind = k.id().to_string();
                                        *name = k.default_name().to_string();
                                    }
                                }
                            });
                        ui.label("in");
                        egui::ComboBox::from_id_salt("nt_tb").selected_text(if timebase == "ticks" { "Ticks" } else { "Samples" }).show_ui(
                            ui,
                            |ui| {
                                ui.selectable_value(timebase, "samples".to_string(), "Samples");
                                ui.selectable_value(timebase, "ticks".to_string(), "Ticks");
                            },
                        );
                        ui.label("Name:");
                        ui.add(egui::TextEdit::singleline(name).desired_width(140.0));
                    });
                    if buttons(ui, "Create", enter) {
                        action =
                            Some(("track.new".into(), json!({"count": count, "format": format, "kind": kind, "timebase": timebase, "name": name})));
                    }
                }
                Dialog::NewSession(n) => {
                    if let Some(params) = new_session_body(app, ui, n, enter) {
                        action = Some(("session.new".into(), params));
                    }
                }
                Dialog::SaveTrackPreset { track, name, chain } => {
                    ui.label(if *chain { "Chain name" } else { "Preset name" });
                    name_field(ui, name);
                    let dir = if *chain { app.dialogs.dirs.chains.clone() } else { app.dialogs.dirs.track_presets.clone() };
                    let file = dir.map(|d| d.join(format!("{}.{}", crate::user_files::file_name(name), crate::user_files::TRACK_PRESET_EXT)));
                    let replaces = file.as_ref().is_some_and(|f| f.exists());
                    if replaces {
                        ui.label(format!("“{}” already exists. Do you want to replace it?", crate::user_files::file_name(name)));
                    }
                    if buttons_enabled(ui, if replaces { "Replace" } else { "Save" }, enter && !replaces, file.is_some() && !name.trim().is_empty())
                        && let Some(f) = file
                    {
                        if let Some(parent) = f.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let cmd = if *chain { "track.save_chain" } else { "track.save_preset" };
                        action = Some((cmd.into(), json!({"path": f.to_string_lossy(), "track": track.0})));
                    }
                }
                Dialog::LoadTrackPreset { tracks, chain, path, presets } => {
                    let cmd = if *chain { "track.load_chain" } else { "track.load_preset" };
                    if presets.is_empty() {
                        let none = if *chain { "No saved plug-in chains yet." } else { "No saved track presets yet." };
                        ui.label(egui::RichText::new(none).color(Tokens::current().text_dim));
                    } else {
                        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                            for p in presets.iter() {
                                let on = std::path::Path::new(path.as_str()) == p.path;
                                if ui.selectable_label(on, p.name.as_str()).clicked() {
                                    *path = p.path.to_string_lossy().into_owned();
                                }
                            }
                        });
                    }
                    ui.horizontal(|ui| {
                        ui.label("File");
                        let hint = if *chain { "a plug-in chain (.json) anywhere" } else { "a track preset (.json) anywhere" };
                        ui.add(egui::TextEdit::singleline(path).desired_width(360.0).hint_text(hint));
                        if let Some(pick) = &app.services.pick_open
                            && ui.button("Browse…").clicked()
                            && let Some(p) = pick(cmd, &[])
                        {
                            *path = p;
                        }
                    });
                    if buttons_enabled(ui, "Load", enter, !path.trim().is_empty()) {
                        action = Some((cmd.into(), json!({"path": path, "tracks": tracks})));
                    }
                }
                Dialog::Bounce { path, format, bit_depth, bitrate, normalize, range } => {
                    ui.horizontal(|ui| {
                        ui.label("File");
                        ui.add(egui::TextEdit::singleline(path).desired_width(420.0));
                        if let Some(pick) = &app.services.pick_save
                            && ui.button("Browse…").clicked()
                            && let Some(p) = pick("file.bounce_mix", path)
                        {
                            *path = p;
                            match std::path::Path::new(path.as_str()).extension().map(|x| x.to_string_lossy().to_lowercase()).as_deref() {
                                Some("wav") => *format = "wav".into(),
                                Some("aif" | "aiff") => *format = "aiff".into(),
                                Some("flac") => *format = "flac".into(),
                                Some("mp3") => *format = "mp3".into(),
                                Some("opus") => *format = "opus".into(),
                                _ => {}
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_label("Format").selected_text(format.to_uppercase()).show_ui(ui, |ui| {
                            for f in ["wav", "aiff", "flac", "mp3", "opus"] {
                                ui.selectable_value(format, f.to_string(), f.to_uppercase());
                            }
                        });
                        // A lossy format has a bitrate where the others have a bit depth.
                        let lossy = soundcraft_audio_io::encode_format_for(format).filter(|f| soundcraft_audio_io::is_lossy(*f));
                        match lossy {
                            Some(f) => {
                                let offered = soundcraft_audio_io::offered_kbps(f);
                                if !offered.contains(bitrate) {
                                    *bitrate = soundcraft_audio_io::default_kbps(f);
                                }
                                egui::ComboBox::from_label("Bitrate").selected_text(format!("{bitrate} kbps")).show_ui(ui, |ui| {
                                    for &k in offered {
                                        ui.selectable_value(bitrate, k, format!("{k} kbps"));
                                    }
                                });
                            }
                            None => {
                                egui::ComboBox::from_label("Bit Depth").selected_text(format!("{bit_depth}-bit")).show_ui(ui, |ui| {
                                    for b in ["16", "24", "32f"] {
                                        ui.selectable_value(bit_depth, b.to_string(), format!("{b}-bit"));
                                    }
                                });
                            }
                        }
                    });
                    if matches!(format.as_str(), "mp3" | "opus") && app.engine.session().main_format().channels() > 2 {
                        ui.label(egui::RichText::new("MP3 and Opus hold stereo: the bounce is folded down").color(Tokens::current().text_dim));
                    }
                    bounce_range_choice(app, ui, range);
                    crate::widgets::checkbox(ui, normalize, "Normalize to -0.1 dBFS");
                    let chosen = range.range(app.engine.session());
                    if buttons_enabled(ui, "Bounce", enter, chosen.is_some())
                        && let Some(r) = chosen
                    {
                        let p = std::path::Path::new(path.as_str()).with_extension(if format == "aiff" { "aif" } else { format.as_str() });
                        let mut params =
                            json!({"path": p.to_string_lossy(), "format": format, "bit_depth": bit_depth, "normalize": normalize, "start": r.start, "end": r.end});
                        if matches!(format.as_str(), "mp3" | "opus") {
                            params["bitrate"] = json!(bitrate);
                            if app.engine.session().main_format().channels() > 2 {
                                params["fold_down"] = json!("stereo");
                            }
                        }
                        crate::bounce::start(app, &params);
                        keep = false;
                    }
                }
                Dialog::RenameTrack { id, name } => {
                    name_field(ui, name);
                    if buttons(ui, "OK", enter) {
                        action = Some(("track.rename".into(), json!({"track": id.0, "name": name})));
                    }
                }
                Dialog::RenameClip { id, name } => {
                    name_field(ui, name);
                    if buttons(ui, "OK", enter) {
                        action = Some(("clip.rename".into(), json!({"clip": id.0, "name": name})));
                    }
                }
                Dialog::RenameMarker { id, name } => {
                    name_field(ui, name);
                    if buttons(ui, "OK", enter) {
                        action = Some(("markers.edit".into(), json!({"id": id.0, "rename": name})));
                    }
                }
                Dialog::PointValue { track, param, at, text } => {
                    name_field(ui, text);
                    let value = crate::edit_window::parse_auto_value(param, text);
                    if value.is_none() {
                        ui.label(egui::RichText::new("Not a value for this lane").color(Tokens::current().text_dim));
                    }
                    if buttons_enabled(ui, "OK", enter, value.is_some())
                        && let Some(v) = value
                    {
                        action = Some(("automation.set_point".into(), json!({"track": track.0, "param": param.id(), "at": at, "value": v})));
                    }
                }
                Dialog::Confirm { message, ok, cmd, params, .. } => {
                    ui.set_max_width(340.0);
                    ui.label(message.as_str());
                    if buttons(ui, ok, enter) {
                        action = Some((cmd.clone(), params.clone()));
                    }
                }
                Dialog::ClearClips { clips, sources, message } => {
                    ui.set_max_width(340.0);
                    ui.label(message.as_str());
                    if buttons(ui, "Remove", enter) {
                        action = Some(("clip.clear".into(), json!({"clips": clips, "sources": sources, "with_clips": true})));
                    }
                }
                Dialog::PathPrompt { cmd, path, key, .. } => {
                    ui.add(egui::TextEdit::singleline(path).desired_width(420.0));
                    if let Some(pick) = &app.services.pick_open
                        && cmd != "session.save_as"
                        && cmd != "session.save_copy"
                        && cmd != "file.export_dawproject"
                        && key != "dir"
                        && ui.button("Browse…").clicked()
                        && let Some(p) = pick(cmd, &[])
                    {
                        *path = p;
                    }
                    if let Some(pick) = &app.services.pick_folder
                        && key == "dir"
                        && ui.button("Browse…").clicked()
                        && let Some(p) = pick(path)
                    {
                        *path = p;
                    }
                    if let Some(pick) = &app.services.pick_save
                        && (cmd == "session.save_as" || cmd == "session.save_copy" || cmd == "file.export_dawproject")
                        && ui.button("Browse…").clicked()
                        && let Some(p) = pick(cmd, path)
                    {
                        *path = p;
                    }
                    let file = soundcraft_engine::io::session_file(path);
                    let replaces = soundcraft_engine::io::replaces(&app.engine, cmd, &file);
                    if replaces {
                        ui.label(format!("“{}” already exists. Do you want to replace it?", file.file_name().unwrap_or_default().to_string_lossy()));
                    }
                    if buttons(ui, if replaces { "Replace" } else { "OK" }, enter && !replaces) {
                        let mut v = json!({});
                        v[key.as_str()] = json!(path);
                        if replaces {
                            v["replace"] = json!(true);
                        }
                        // Saved templates are listed in File › New from their folder: made on save.
                        if cmd == "session.save_template"
                            && let Some(dir) = app.dialogs.dirs.templates.as_deref()
                            && file.parent() == Some(dir)
                        {
                            let _ = std::fs::create_dir_all(dir);
                        }
                        action = Some((cmd.clone(), v));
                    }
                }
                Dialog::Number { cmd, key, value, suffix, .. } => {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(value).speed(0.1));
                        ui.label(suffix.as_str());
                    });
                    if buttons(ui, "OK", enter) {
                        let v = if key == "by_seconds" { json!({"by": {"seconds": value}}) } else { json!({key.as_str(): value}) };
                        action = Some((cmd.clone(), v));
                    }
                }
                Dialog::TempoChange { at, bpm } => {
                    ui.horizontal(|ui| {
                        // Same limits the engine enforces (time crate `valid_bpm`).
                        ui.add(egui::DragValue::new(bpm).speed(0.1).range(5.0..=1000.0));
                        ui.label("bpm");
                    });
                    if buttons(ui, "OK", enter) {
                        action = Some(tempo_change_action(*at, *bpm));
                    }
                }
                Dialog::KeyPicker { at, edit, root, minor } => {
                    crate::key_ruler::picker_body(ui, root, minor);
                    if buttons(ui, "OK", enter) {
                        let key = crate::session_key::spelled_key(*root, *minor);
                        action = Some(if *edit {
                            ("event.key_change_edit".into(), json!({"at": at, "key": key}))
                        } else {
                            ("event.add_key_change".into(), json!({"at": at, "key": key}))
                        });
                    }
                }
                Dialog::ChordPicker(p) => {
                    let s = app.engine.session_arc();
                    let held = app.midi_hub.as_ref().map(|h| h.held_notes()).unwrap_or_default();
                    // Keys played in are named as they go down, idle window or not.
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
                    action = crate::band::chord_picker::body(ui, &s, p, &held, |ui, ready| buttons_enabled(ui, "OK", enter, ready));
                    // A chord played in moves the picker on to the next slot once it is added.
                    if action.is_some() && p.from_midi && p.chord.is_none() {
                        chord_next = Some(s.tempo.samples_to_ticks(p.end, s.sample_rate));
                    }
                }
                Dialog::Spot(d) => {
                    crate::spot::fields(ui, app.engine.session(), d);
                    if buttons(ui, "OK", enter) {
                        keep = false;
                        action = d.command(app.engine.session());
                    }
                }
                Dialog::Fades { shape } => {
                    for s in soundcraft_model::FadeShape::ALL {
                        ui.radio_value(shape, s.label().to_lowercase(), s.label());
                    }
                    if buttons(ui, "OK", enter) {
                        action = Some(("edit.fades_create".into(), json!({"shape": shape})));
                    }
                }
                Dialog::ChangeVelocity(d) => {
                    velocity_fields(ui, d);
                    if buttons(ui, "OK", enter) {
                        action = Some(velocity_action(app, d));
                    }
                }
                Dialog::StripSilence { threshold, min_ms, pre_ms, post_ms } => {
                    ui.add(egui::Slider::new(threshold, -96.0..=0.0).text("Threshold dB"));
                    ui.add(egui::Slider::new(min_ms, 0.0..=2000.0).text("Min strip duration ms"));
                    ui.add(egui::Slider::new(pre_ms, 0.0..=500.0).text("Clip start pad ms"));
                    ui.add(egui::Slider::new(post_ms, 0.0..=2000.0).text("Clip end pad ms"));
                    if buttons(ui, "Strip", enter) {
                        action = Some((
                            "edit.strip_silence".into(),
                            json!({"threshold_db": threshold, "min_length_ms": min_ms, "pad_before_ms": pre_ms, "pad_after_ms": post_ms}),
                        ));
                    }
                }
                Dialog::Group { id, name, edit, mix, members } => {
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        ui.text_edit_singleline(name);
                    });
                    ui.horizontal(|ui| {
                        crate::widgets::checkbox(ui, edit, "Edit group");
                        crate::widgets::checkbox(ui, mix, "Mix group");
                    });
                    ui.label("Tracks");
                    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                        for (_, n, on) in members.iter_mut() {
                            crate::widgets::checkbox(ui, on, n.as_str());
                        }
                    });
                    let ids: Vec<u64> = members.iter().filter(|m| m.2).map(|m| m.0).collect();
                    // Both commands reject an empty group, so don't offer an OK that can only fail.
                    if buttons_enabled(ui, "OK", enter, !ids.is_empty()) {
                        let mut params = json!({"name": name, "edit": edit, "mix": mix, "tracks": ids});
                        let cmd = match id {
                            Some(g) => {
                                params["group"] = json!(g.0);
                                "track.group_modify"
                            }
                            None => "track.group",
                        };
                        action = Some((cmd.into(), params));
                    }
                }
                Dialog::Session { frame_rate, bit_depth } => {
                    egui::ComboBox::from_label("Timecode Rate").selected_text(frame_rate.as_str()).show_ui(ui, |ui| {
                        for r in soundcraft_time::FrameRate::ALL {
                            ui.selectable_value(frame_rate, r.label().to_string(), r.label());
                        }
                    });
                    egui::ComboBox::from_label("Bit Depth").selected_text(bit_depth.as_str()).show_ui(ui, |ui| {
                        for b in ["16", "24", "32f"] {
                            ui.selectable_value(bit_depth, b.to_string(), b);
                        }
                    });
                    if buttons(ui, "OK", enter) {
                        action = Some(("setup.session".into(), json!({"frame_rate": frame_rate, "bit_depth": bit_depth})));
                    }
                }
                Dialog::ScoreSetup { title, composer, bars_per_system, show_track_names } => {
                    egui::Grid::new("score_setup").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
                        ui.label("Title");
                        ui.text_edit_singleline(title);
                        ui.end_row();
                        ui.label("Composer");
                        ui.text_edit_singleline(composer);
                        ui.end_row();
                        ui.label("Bars per system");
                        ui.add(egui::DragValue::new(bars_per_system).range(1..=16));
                        ui.end_row();
                    });
                    crate::widgets::checkbox(ui, show_track_names, "Show track names");
                    if buttons(ui, "OK", enter) {
                        action = Some((
                            "file.score_setup".into(),
                            json!({"title": title, "composer": composer, "bars_per_system": bars_per_system, "show_track_names": show_track_names}),
                        ));
                    }
                }
                Dialog::PrePostRoll { on, pre, post } => {
                    crate::widgets::checkbox(ui, on, "Pre/Post-Roll on");
                    egui::Grid::new("pre_post_roll").num_columns(2).spacing(vec2(10.0, 8.0)).show(ui, |ui| {
                        ui.label("Pre-roll");
                        ui.add(egui::DragValue::new(pre).range(0.0..=60.0).speed(0.05).max_decimals(2).suffix(" s"));
                        ui.end_row();
                        ui.label("Post-roll");
                        ui.add(egui::DragValue::new(post).range(0.0..=60.0).speed(0.05).max_decimals(2).suffix(" s"));
                        ui.end_row();
                    });
                    if buttons(ui, "OK", enter) {
                        keep = false;
                        let steps = [
                            ("options.pre_roll", json!({"length": {"seconds": pre}})),
                            ("options.post_roll", json!({"length": {"seconds": post}})),
                            ("options.pre_post_roll", json!({"value": on})),
                        ];
                        for (cmd, params) in steps {
                            if let Err(e) = app.run(cmd, params) {
                                app.ui.status = e;
                            }
                        }
                    }
                }
                Dialog::SaveChanges { then } => {
                    let name = &app.engine.session().name;
                    let revert = then.0 == "session.revert";
                    if revert || cfg!(target_arch = "wasm32") {
                        let (question, ok) = if revert {
                            (format!("Revert “{name}” to its last saved version?"), "Revert")
                        } else {
                            (format!("Close “{name}” without saving?"), "Don't Save")
                        };
                        ui.label(format!("{question} Your changes will be lost."));
                        if buttons(ui, ok, false) {
                            action = Some(then.clone());
                        }
                    } else {
                        ui.label(format!("Save changes to “{name}” before closing?"));
                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("Don't Save").clicked() {
                                action = Some(then.clone());
                            }
                            if buttons(ui, "Save", enter) {
                                save_first = Some(then.clone());
                            }
                        });
                    }
                }
            }
        });
    if esc || ctx.memory(|m| m.data.get_temp::<bool>(egui::Id::new("dlg_cancel")).unwrap_or(false)) {
        keep = false;
        app.dialogs.after_save = None;
        app.dialogs.pending_session = None;
        ctx.memory_mut(|m| m.data.remove::<bool>(egui::Id::new("dlg_cancel")));
    }
    if let Some(then) = save_first {
        keep = false;
        if !app.dialogs.open_for_command(&app.engine, "session.save") {
            action = Some(("session.save".into(), json!({})));
        }
        app.dialogs.after_save = Some(then);
    }
    if let Some((cmd, params)) = &action
        && !confirmed
        && ask_to_save(app, cmd, params)
    {
        keep = false;
        action = None;
    }
    let saving = action.as_ref().is_some_and(|(cmd, _)| cmd.starts_with("session.save"));
    if let Some((cmd, params)) = action {
        keep = false;
        match run_then(app, &cmd, params) {
            Ok(v) if cmd.starts_with("band.chord.") => {
                if v.get("exact").and_then(Value::as_bool) == Some(false)
                    && let Some(id) = v.get("chord").and_then(Value::as_u64).and_then(|c| u32::try_from(c).ok())
                {
                    ctx.data_mut(|m| m.insert_temp(egui::Id::new(crate::band::chord_picker::FLASH_REQUEST), id));
                }
                if let Some(at) = chord_next {
                    crate::band::chord_picker::open(app, at, None);
                    if let Some(Dialog::ChordPicker(next)) = &mut app.dialogs.open {
                        next.from_midi = true;
                        next.held = app.midi_hub.as_ref().map(|h| h.held_notes()).unwrap_or_default();
                    }
                }
            }
            Ok(v) if cmd == "file.export_clips" => {
                app.ui.status = format!("Exported {} clip(s) to {}", v["written"], v["dir"].as_str().unwrap_or_default());
            }
            Ok(_) => {}
            Err(e) => app.ui.status = e,
        }
    }
    if saving
        && let Some((then, params)) = app.dialogs.after_save.take()
        && !app.engine.is_dirty()
        && let Err(e) = run_then(app, &then, params)
    {
        app.ui.status = e;
    }
    if keep {
        app.dialogs.open = Some(d);
    }
}

/// Runs a dialog's command, or what `SaveChanges` carries on with.
fn run_then(app: &mut SoundApp, cmd: &str, params: Value) -> Result<Value, String> {
    if cmd == OPEN_PENDING {
        open_pending_session(app);
        return Ok(Value::Null);
    }
    app.run(cmd, params)
}

/// The New dialog (the approved mockup): Name, Sample rate, the Start from list (Blank, Demo,
/// then My templates, newest first), the chosen template's details and Browse…. Returns
/// `session.new`'s parameters when Create is pressed (or a row is double-clicked).
fn new_session_body(app: &mut SoundApp, ui: &mut egui::Ui, n: &mut NewSession, enter: bool) -> Option<Value> {
    use crate::user_files::{template_details, template_info};
    let t = Tokens::current();
    let template = match &n.start {
        Start::Template(p) => Some(p.clone()),
        _ => None,
    };
    // The details line, worked out once per choice (it reads the template file).
    if let Some(p) = &template
        && n.details.as_ref().is_none_or(|(q, _)| q != p)
    {
        let modified = std::fs::metadata(p).and_then(|m| m.modified()).ok();
        n.details = Some((p.clone(), template_details(template_info(p), modified)));
    }
    let info = template.as_ref().and_then(|p| n.details.as_ref().filter(|(q, _)| q == p).map(|(_, d)| d.clone()));
    ui.set_width(560.0);
    egui::Grid::new("new_session_grid").num_columns(2).spacing(vec2(24.0, 10.0)).show(ui, |ui| {
        ui.label("Name");
        ui.add(egui::TextEdit::singleline(&mut n.name).desired_width(330.0));
        ui.end_row();
        ui.label("Sample rate");
        ui.add_enabled_ui(template.is_none(), |ui| {
            egui::ComboBox::from_id_salt("new_session_rate").width(150.0).selected_text(format!("{} Hz", n.sample_rate)).show_ui(ui, |ui| {
                for r in soundcraft_time::SampleRate::COMMON {
                    ui.selectable_value(&mut n.sample_rate, r, format!("{r} Hz"));
                }
            });
        })
        .response
        .on_disabled_hover_text("A template keeps the sample rate it was saved at");
        ui.end_row();
    });
    ui.add_space(4.0);
    ui.label(egui::RichText::new("START FROM").small().strong().color(t.text_dim));
    // Up and Down walk the list (New from Template… opens with it in hand).
    let order: Vec<Start> = [Start::Blank, Start::Demo].into_iter().chain(n.templates.iter().map(|f| Start::Template(f.path.clone()))).collect();
    if !ui.ctx().egui_wants_keyboard_input() {
        let step = ui.input(|i| i32::from(i.key_pressed(egui::Key::ArrowDown)) - i32::from(i.key_pressed(egui::Key::ArrowUp)));
        if step != 0 {
            let at = order.iter().position(|o| *o == n.start).unwrap_or(0);
            let to = if step > 0 { at.saturating_add(1).min(order.len().saturating_sub(1)) } else { at.saturating_sub(1) };
            if let Some(o) = order.get(to) {
                n.start = o.clone();
            }
        }
    }
    let mut chosen: Option<Start> = None;
    let mut create = false;
    let selected_fill = ui.visuals().selection.bg_fill.gamma_multiply(0.6);
    let hover_fill = ui.visuals().widgets.hovered.weak_bg_fill;
    egui::Frame::new().fill(t.panel_bg).stroke(Stroke::new(1.0, t.border)).corner_radius(3.0).inner_margin(egui::Margin::symmetric(0, 6)).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::vertical().max_height(220.0).min_scrolled_height(220.0).auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut row = |ui: &mut egui::Ui, start: Start, name: &str, right: &str| {
                    let selected = n.start == start;
                    let w = ui.available_width();
                    let (r, resp) = ui.allocate_exact_size(vec2(w, 27.0), egui::Sense::click());
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, name));
                    if selected {
                        ui.painter().rect_filled(r, 0.0, selected_fill);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(r, 0.0, hover_fill);
                    }
                    let font = if selected { bold(13.0) } else { crate::theme::regular(13.0) };
                    ui.painter().text(pos2(r.min.x + 14.0, r.center().y), Align2::LEFT_CENTER, name, font, t.text);
                    ui.painter().text(pos2(r.max.x - 14.0, r.center().y), Align2::RIGHT_CENTER, right, crate::theme::regular(11.5), t.text_dim);
                    if resp.clicked() {
                        chosen = Some(start.clone());
                    }
                    if resp.double_clicked() {
                        chosen = Some(start);
                        create = true;
                    }
                };
                row(ui, Start::Blank, "Blank", "empty project");
                row(ui, Start::Demo, "Demo", "example session");
                // The divider.
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), egui::Sense::hover());
                let g = ui.painter().layout_no_wrap("MY TEMPLATES".into(), bold(10.0), t.text_dim);
                let gw = g.size().x;
                ui.painter().galley(pos2(r.min.x + 14.0, r.center().y - g.size().y / 2.0), g, t.text_dim);
                ui.painter().line_segment([pos2(r.min.x + 22.0 + gw, r.center().y), pos2(r.max.x - 14.0, r.center().y)], Stroke::new(1.0, t.border));
                if n.templates.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        ui.label(egui::RichText::new("No saved templates yet: File › Save As Template… adds one.").color(t.text_dim));
                    });
                }
                for f in &n.templates {
                    let date = f.modified.map(crate::user_files::short_date).unwrap_or_default();
                    row(ui, Start::Template(f.path.clone()), &f.name, &date);
                }
                // A file browsed to, outside the templates folder.
                if let Some(p) = template.as_ref().filter(|p| !n.templates.iter().any(|f| &f.path == *p)) {
                    let name = p.file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
                    row(ui, Start::Template(p.clone()), &name, "browsed");
                }
            });
        },
    );
    if let Some(c) = chosen {
        n.start = c;
    }
    ui.horizontal(|ui| {
        match &info {
            Some(d) => ui.label(egui::RichText::new(d).color(t.accent)),
            None => ui.label(""),
        };
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.link("Browse…").clicked() {
                let picked = app.services.pick_open.as_ref().and_then(|pick| pick("session.new", &[]));
                match picked {
                    Some(p) => n.start = Start::Template(p.into()),
                    None => n.browse = Some(n.browse.take().unwrap_or_default()),
                }
            }
        });
    });
    if let Some(path) = &mut n.browse {
        let mut use_it = false;
        ui.horizontal(|ui| {
            ui.label("Template file");
            let r = ui.add(egui::TextEdit::singleline(path).desired_width(360.0).hint_text("a .scraft template anywhere"));
            let typed = !path.trim().is_empty();
            use_it = ui.add_enabled(typed, egui::Button::new("Use")).clicked()
                || (typed && r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
        });
        if use_it {
            n.start = Start::Template(path.trim().into());
            n.browse = None;
            return None;
        }
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.label(egui::RichText::new("Also available from ").small().color(t.text_dim));
        ui.label(egui::RichText::new("File › New from Template…").small().strong().color(t.text_dim));
        ui.label(egui::RichText::new(" (same list).").small().color(t.text_dim));
    });
    (buttons(ui, "Create", enter && n.browse.is_none()) || create).then(|| n.params())
}

/// Cancel/OK row; returns true when OK was pressed (or Enter).
fn buttons(ui: &mut egui::Ui, ok: &str, enter: bool) -> bool {
    buttons_enabled(ui, ok, enter, true)
}

/// `buttons`, but a greyed-out OK when the dialog's values can't be submitted yet. Cancel always works.
fn buttons_enabled(ui: &mut egui::Ui, ok: &str, enter: bool, ready: bool) -> bool {
    let mut pressed = enter && ready;
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("Cancel").clicked() {
            ui.ctx().memory_mut(|m| m.data.insert_temp(egui::Id::new("dlg_cancel"), true));
        }
        let b = egui::Button::new(egui::RichText::new(ok).strong()).fill(Tokens::current().accent);
        if ui.add_enabled(ready, b).clicked() {
            pressed = true;
        }
    });
    pressed
}

/// Set once the open dialog's name field has selected its text.
const NAME_SELECTED: &str = "dialog-name-selected";

/// A name field that takes the focus with its text selected, so typing replaces the old name. It
/// takes the focus back in the other window when the dialog moves there, keeping the text.
fn name_field(ui: &mut egui::Ui, name: &mut String) {
    let mut out = egui::TextEdit::singleline(name).show(ui);
    if !out.response.has_focus() {
        out.response.request_focus();
        let selected = egui::Id::new(NAME_SELECTED);
        if !ui.ctx().data(|d| d.get_temp::<bool>(selected).unwrap_or(false)) {
            let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(name.chars().count()));
            out.state.cursor.set_char_range(Some(all));
            out.state.store(ui.ctx(), out.response.id);
            ui.ctx().data_mut(|d| d.insert_temp(selected, true));
        }
    }
}

/// The command the tempo-change dialog runs on OK.
fn tempo_change_action(at: Samples, bpm: f64) -> (String, Value) {
    ("event.tempo".into(), json!({"bpm": bpm, "at": at}))
}

fn audiosuite_window(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(process) = app.ui.audiosuite.clone() else { return };
    let mut open = true;
    let title = soundcraft_dsp::plugin_info(&process).map_or_else(|| process.replace('_', " "), |p| p.name.to_string());
    egui::Window::new(format!("Process · {title}")).open(&mut open).default_width(360.0).show(ctx, |ui| {
        let key = egui::Id::new(("as_params", process.clone()));
        let mut params: std::collections::BTreeMap<String, f32> = ui.ctx().memory(|m| m.data.get_temp(key)).unwrap_or_default();
        if let Some(info) = soundcraft_dsp::plugin_info(&process) {
            for p in info.params {
                let v = params.entry(p.id.to_string()).or_insert(p.default);
                ui.add(egui::Slider::new(v, p.min..=p.max).text(p.name));
            }
        } else {
            match process.as_str() {
                "normalize" => {
                    let v = params.entry("target_db".into()).or_insert(-0.1);
                    ui.add(egui::Slider::new(v, -24.0..=0.0).text("Peak dB"));
                }
                "time_stretch" => {
                    let v = params.entry("ratio".into()).or_insert(1.0);
                    ui.add(egui::Slider::new(v, 0.25..=4.0).text("Length ratio"));
                }
                "pitch_shift" => {
                    let v = params.entry("semitones".into()).or_insert(0.0);
                    ui.add(egui::Slider::new(v, -24.0..=24.0).text("Semitones"));
                }
                "varispeed" => {
                    let v = params.entry("speed".into()).or_insert(1.0);
                    ui.add(egui::Slider::new(v, 0.25..=4.0).text("Speed"));
                }
                _ => {
                    ui.label("No parameters.");
                }
            }
        }
        ui.ctx().memory_mut(|m| m.data.insert_temp(key, params.clone()));
        let hint = if soundcraft_dsp::is_generator(&process) {
            "Fills the selection on the selected tracks with a new clip."
        } else {
            "Processes the selected clips and replaces them with rendered audio."
        };
        ui.label(egui::RichText::new(hint).small());
        if ui.button(egui::RichText::new("Render").strong()).clicked() {
            let p: serde_json::Map<String, Value> = params.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
            let _ = app.run("process.run", json!({"process": process, "params": p}));
        }
    });
    if !open {
        app.ui.audiosuite = None;
    }
}

/// The Range rows of the Bounce Mix dialog: Loop, Between markers, Whole session.
fn bounce_range_choice(app: &SoundApp, ui: &mut egui::Ui, range: &mut BounceRange) {
    let t = Tokens::current();
    let s = app.engine.session();
    let fmt = s.edit.main_counter;
    let dim = |text: String| egui::RichText::new(text).small().color(t.text_dim);
    ui.add_space(4.0);
    ui.label("Range");
    ui.indent("bounce_range", |ui| {
        // Loop.
        ui.horizontal(|ui| match s.edit.loop_range.filter(|r| !r.is_empty()) {
            Some(lp) => {
                ui.radio_value(range, BounceRange::Loop, "Loop");
                let secs = lp.len() as f64 / s.sample_rate.as_f64();
                ui.label(dim(format!("{} – {} ({secs:.1} s)", s.format_position(lp.start, fmt), s.format_position(lp.end, fmt))));
            }
            None => {
                ui.add_enabled(false, egui::RadioButton::new(false, "Loop"));
                ui.label(dim("no loop (select a range first)".into()));
            }
        });
        // Between markers.
        let markers = BounceRange::markers(s);
        ui.horizontal(|ui| {
            let Some(first) = markers.first() else {
                ui.add_enabled(false, egui::RadioButton::new(false, "Between markers"));
                ui.label(dim("no markers".into()));
                return;
            };
            let (from, to) = match range {
                BounceRange::Markers { from, to } => (*from, *to),
                _ => (first.id, markers.get(1).map(|m| m.id)),
            };
            if ui.radio(matches!(range, BounceRange::Markers { .. }), "Between markers").clicked() {
                *range = BounceRange::Markers { from, to };
            }
            let name = |id: Option<soundcraft_model::MarkerId>| match id {
                Some(id) => markers.iter().find(|m| m.id == id).map_or_else(|| "?".to_string(), |m| m.name.clone()),
                None => "End of session".to_string(),
            };
            let (mut nf, mut nt) = (from, to);
            egui::ComboBox::from_id_salt("bounce_from").selected_text(name(Some(nf))).show_ui(ui, |ui| {
                for m in &markers {
                    ui.selectable_value(&mut nf, m.id, m.name.as_str());
                }
            });
            ui.label("to");
            let from_at = markers.iter().find(|m| m.id == nf).map_or(0, |m| m.start);
            let after: Vec<_> = markers.iter().filter(|m| m.id != nf && m.start >= from_at).collect();
            if nt.is_some_and(|id| !after.iter().any(|m| m.id == id)) {
                nt = after.first().map(|m| m.id);
            }
            egui::ComboBox::from_id_salt("bounce_to").selected_text(name(nt)).show_ui(ui, |ui| {
                for m in &after {
                    ui.selectable_value(&mut nt, Some(m.id), m.name.as_str());
                }
                ui.selectable_value(&mut nt, None, "End of session");
            });
            // A change in either picker chooses Between markers.
            if (nf, nt) != (from, to) {
                *range = BounceRange::Markers { from: nf, to: nt };
            }
        });
        // Whole session.
        ui.horizontal(|ui| {
            ui.radio_value(range, BounceRange::Whole, "Whole session");
            let end = s.content_end().max(1);
            ui.label(dim(format!("{} – {}", s.format_position(0, fmt), s.format_position(end, fmt))));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_commands_get_path_prompts_with_sensible_names() {
        let e = soundcraft_engine::demo::demo_engine();
        let mut d = Dialogs::default();
        for (id, ext) in [("file.export_sibelius", ".musicxml"), ("file.print_score", ".svg"), ("file.export_clip_groups", ".scgrp")] {
            assert!(takes_path(id), "{id}");
            assert!(d.open_for_command(&e, id), "{id}");
            match d.open.take() {
                Some(Dialog::PathPrompt { cmd, path, key, .. }) => {
                    assert_eq!(cmd, id);
                    assert_eq!(key, "path");
                    assert!(path.ends_with(ext), "{path}");
                }
                other => panic!("{id}: {other:?}"),
            }
        }
        // Optional-path commands run directly.
        assert!(!takes_path("file.send_to_sibelius"));
        assert!(!takes_path("edit.copy"));
        assert!(d.open_for_command(&e, "file.score_setup"));
        assert!(matches!(d.open, Some(Dialog::ScoreSetup { bars_per_system: 4, .. })));
    }

    #[test]
    fn clip_rename_opens_the_rename_dialog_for_the_selected_clip() {
        let mut e = soundcraft_engine::demo::demo_engine();
        let mut d = Dialogs::default();
        e.execute("edit.select_none", &json!({})).unwrap();
        assert!(!d.open_for_command(&e, "clip.rename"));
        let clip = e.session().track_by_name("Kick").unwrap().clips()[0].clone();
        e.execute("edit.select", &json!({"clips": [clip.id.0]})).unwrap();
        assert!(d.open_for_command(&e, "clip.rename"));
        assert!(matches!(d.open, Some(Dialog::RenameClip { id, ref name }) if id == clip.id && *name == clip.name));
    }

    #[test]
    fn tempo_change_dialog_runs_a_real_command_at_its_position() {
        let (cmd, params) = tempo_change_action(96_000, 133.5);
        assert!(soundcraft_engine::command_specs().iter().any(|c| c.id == cmd), "{cmd} is not a command");
        assert_eq!(params["at"], 96_000);
        assert_eq!(params["bpm"], 133.5);
    }

    #[test]
    fn clear_always_asks_and_names_the_timeline_clips_that_go_too() {
        let mut e = soundcraft_engine::demo::demo_engine();
        let file = |e: &Engine, name: &str| e.session().sources.iter().find(|f| f.name == name).map(|f| f.id).unwrap();
        let (kick, hats) = (file(&e, "Kick"), file(&e, "Hats"));
        let kick_clips: Vec<ClipId> = e.session().clips_using(kick).map(|c| c.id).collect();
        let snare = e.session().track_by_name("Snare").unwrap().clips()[0].id;
        let message = |d: &Dialogs| match &d.open {
            Some(Dialog::ClearClips { message, .. }) => message.clone(),
            other => panic!("{other:?}"),
        };
        let mut d = Dialogs::default();
        assert!(!d.open_for_command(&e, "clip.clear"));
        e.execute("edit.select", &json!({"clips": [snare]})).unwrap();
        assert!(d.open_for_command(&e, "clip.clear"));
        assert_eq!(message(&d), "Remove the clip \"Snare\" from the session?");
        e.execute("edit.select", &json!({"sources": [kick]})).unwrap();
        assert!(d.open_for_command(&e, "clip.clear"));
        assert!(matches!(&d.open, Some(Dialog::ClearClips { clips, sources, .. }) if clips.is_empty() && *sources == vec![kick]));
        assert_eq!(message(&d), "Remove the file \"Kick\" from the session? It is used by 3 clips on the timeline, which are removed too.");
        assert!(d.open_clear(&e, kick_clips.clone(), vec![kick]));
        assert_eq!(message(&d), "Remove 3 clips and 1 file from the session?");
        assert!(d.open_clear(&e, kick_clips, vec![kick, hats]));
        assert_eq!(message(&d), "Remove 3 clips and 2 files from the session? \"Hats\" is used by a clip on the timeline, which is removed too.");
        e.session_mut().sources.iter_mut().for_each(|f| f.unsaved = false);
        assert!(d.open_clear(&e, vec![], vec![kick, hats]));
        assert_eq!(
            message(&d),
            "Remove 2 files from the session? They are used by 4 clips on the timeline, which are removed too. The files stay on disk."
        );
    }
}

/// How Change Velocity… changes the velocities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VelocityMode {
    Set,
    Add,
    Scale,
    Ramp,
}

/// The Change Velocity… dialog's fields.
#[derive(Debug, Clone, PartialEq)]
pub struct VelocityDialog {
    pub mode: VelocityMode,
    pub set: i64,
    pub add: i64,
    /// Percent.
    pub scale: f64,
    pub from: i64,
    pub to: i64,
}

impl Default for VelocityDialog {
    fn default() -> Self {
        VelocityDialog { mode: VelocityMode::Set, set: 100, add: 10, scale: 100.0, from: 40, to: 110 }
    }
}

impl VelocityDialog {
    /// The change as command parameters (`set`, `add`, `scale` or `from`/`to`).
    pub fn params(&self) -> Value {
        match self.mode {
            VelocityMode::Set => json!({"set": self.set.clamp(1, 127)}),
            VelocityMode::Add => json!({"add": self.add.clamp(-127, 127)}),
            VelocityMode::Scale => json!({"scale": (self.scale / 100.0).clamp(0.0, 10.0)}),
            VelocityMode::Ramp => json!({"from": self.from.clamp(1, 127), "to": self.to.clamp(1, 127)}),
        }
    }
}

fn velocity_fields(ui: &mut egui::Ui, d: &mut VelocityDialog) {
    ui.horizontal(|ui| {
        ui.radio_value(&mut d.mode, VelocityMode::Set, "Set to");
        ui.add_enabled(d.mode == VelocityMode::Set, egui::DragValue::new(&mut d.set).range(1..=127));
    });
    ui.horizontal(|ui| {
        ui.radio_value(&mut d.mode, VelocityMode::Add, "Add");
        ui.add_enabled(d.mode == VelocityMode::Add, egui::DragValue::new(&mut d.add).range(-127..=127));
    });
    ui.horizontal(|ui| {
        ui.radio_value(&mut d.mode, VelocityMode::Scale, "Scale by");
        ui.add_enabled(d.mode == VelocityMode::Scale, egui::DragValue::new(&mut d.scale).range(0.0..=1000.0).suffix(" %"));
    });
    ui.horizontal(|ui| {
        ui.radio_value(&mut d.mode, VelocityMode::Ramp, "Ramp from");
        let on = d.mode == VelocityMode::Ramp;
        ui.add_enabled(on, egui::DragValue::new(&mut d.from).range(1..=127));
        ui.label("to");
        ui.add_enabled(on, egui::DragValue::new(&mut d.to).range(1..=127));
    });
}

/// What Change Velocity… runs: on the notes selected in the MIDI Editor when it shows them,
/// else on every note of the selected MIDI clips.
pub fn velocity_action(app: &SoundApp, d: &VelocityDialog) -> (String, Value) {
    let mut p = d.params();
    let editing = app.ui.show_midi_editor.then(|| crate::midi_editor::target_clip(app)).flatten();
    match editing {
        Some(clip) if !app.midi.selected.is_empty() && app.midi.clip == Some(clip) => {
            p["clip"] = json!(clip.0);
            p["indices"] = json!(app.midi.selected);
            ("midi.notes_velocity".into(), p)
        }
        _ => ("event.change_velocity".into(), p),
    }
}
