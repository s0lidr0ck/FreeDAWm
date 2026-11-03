//! Replacing the session in two halves, so a host can show progress instead of freezing
//! (spec `docs/superpowers/specs/2026-10-28-open-progress.md`): [`load_session`] does the slow,
//! thread-safe work (reading the file, decoding its audio, building waveforms) anywhere, reporting
//! each step through a callback; [`install_loaded`] puts the result in the engine, on the thread
//! that owns it. `session.open`, `session.new`, `session.close` and `session.revert` run both in
//! one go; a UI runs the first on a worker, creates the hosted plugin instances a frame at a time
//! ("Plugins", which only a host with a player does) and installs last ("Finishing").

use crate::{Engine, Result};
use serde_json::{Value, json};
use soundcraft_model::Session;
use soundcraft_time::SampleRate;
use std::path::Path;

/// The phases of an open, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OpenPhase {
    #[default]
    Reading,
    Audio,
    Waveforms,
    Plugins,
    Finishing,
}

impl OpenPhase {
    /// Every phase, in order.
    pub const ALL: [OpenPhase; 5] = [OpenPhase::Reading, OpenPhase::Audio, OpenPhase::Waveforms, OpenPhase::Plugins, OpenPhase::Finishing];

    /// The line the progress window shows for the phase.
    pub fn label(self) -> &'static str {
        match self {
            OpenPhase::Reading => "Reading session",
            OpenPhase::Audio => "Loading audio",
            OpenPhase::Waveforms => "Waveforms",
            OpenPhase::Plugins => "Plugins",
            OpenPhase::Finishing => "Finishing",
        }
    }

    /// The phase's share of the bar (they sum to 1).
    pub fn weight(self) -> f32 {
        match self {
            OpenPhase::Reading => 0.05,
            OpenPhase::Audio => 0.25,
            OpenPhase::Waveforms => 0.10,
            OpenPhase::Plugins => 0.55,
            OpenPhase::Finishing => 0.05,
        }
    }

    /// Where the bar stands at the start of the phase.
    pub fn start(self) -> f32 {
        OpenPhase::ALL.iter().take_while(|p| **p != self).map(|p| p.weight()).sum()
    }
}

/// Where an open stands: the phase, how many of its items are done, and the item under way.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OpenProgress {
    pub phase: OpenPhase,
    pub done: usize,
    pub total: usize,
    /// The file or plugin under way (empty when the phase has no items).
    pub item: String,
}

impl OpenProgress {
    pub fn new(phase: OpenPhase, done: usize, total: usize, item: impl Into<String>) -> Self {
        OpenProgress { phase, done, total, item: item.into() }
    }

    /// The bar's fill, 0..=1, weighted by phase: a phase without items counts as begun.
    pub fn fraction(&self) -> f32 {
        let within = if self.total == 0 { 0.0 } else { (self.done as f32 / self.total as f32).clamp(0.0, 1.0) };
        (self.phase.start() + self.phase.weight() * within).clamp(0.0, 1.0)
    }

    /// The phase line with its count: "Loading audio (2 of 7)".
    pub fn phase_line(&self) -> String {
        if self.total == 0 {
            self.phase.label().to_string()
        } else {
            format!("{} ({} of {})", self.phase.label(), self.done.min(self.total), self.total)
        }
    }
}

/// What a session-replacing command asked for, worked out from the engine before the load.
#[derive(Debug, Clone, PartialEq)]
pub enum OpenRequest {
    /// `session.open {path}`.
    Open { path: String },
    /// `session.revert`: the engine's own file.
    Revert { path: String },
    /// `session.new {template: <file>}`: the template as a new untitled session `name`.
    Template { path: String, name: String },
    /// `session.new` (blank or the demo) and `session.close`: nothing to read.
    Fresh { session: Box<Session> },
    /// `file.import_dawproject {path}`: a DAWproject file as a new untitled session.
    DawProject { path: String },
}

impl OpenRequest {
    /// The request for `id` with `params` (one of `session.open`, `session.new`, `session.close`,
    /// `session.revert`); `None` for any other command.
    pub fn from_command(e: &Engine, id: &str, p: &Value) -> Option<Result<OpenRequest>> {
        use crate::cmd::{bad, i64_or, str_param};
        Some(match id {
            "session.open" => str_param(p, "path").map(|path| OpenRequest::Open { path: path.to_string() }).ok_or_else(|| bad(id, "`path` required")),
            "session.revert" => e.path.clone().map(|path| OpenRequest::Revert { path }).ok_or_else(|| bad(id, "the session has not been saved")),
            "session.close" => Ok(OpenRequest::Fresh { session: Box::default() }),
            "file.import_dawproject" => match str_param(p, "path") {
                Some(path) if crate::io::file_found(path) => Ok(OpenRequest::DawProject { path: path.to_string() }),
                Some(path) => Err(bad(id, format!("no file at {path}"))),
                None => Err(bad(id, "`path` required")),
            },
            "session.new" => (|| {
                let name = str_param(p, "name").unwrap_or("Untitled").to_string();
                let sr =
                    SampleRate::new(i64_or(p, "sample_rate", 48_000).clamp(0, i64::from(u32::MAX)) as u32).map_err(|err| bad(id, err.to_string()))?;
                let mut s = match str_param(p, "template").map(str::trim) {
                    Some("demo") => crate::demo::demo_session(),
                    None | Some("" | "blank") => Session::new(name.clone(), sr),
                    Some(path) => {
                        if !crate::io::file_found(path) {
                            return Err(bad(id, format!("no template file at {path}")));
                        }
                        return Ok(OpenRequest::Template { path: path.to_string(), name });
                    }
                };
                s.name = name;
                if let Some(b) = p.get("bit_depth") {
                    s.bit_depth = match b.to_string().trim_matches('"') {
                        "16" => soundcraft_model::BitDepthSetting::Int16,
                        "32" | "32f" => soundcraft_model::BitDepthSetting::Float32,
                        _ => soundcraft_model::BitDepthSetting::Int24,
                    };
                }
                Ok(OpenRequest::Fresh { session: Box::new(s) })
            })(),
            _ => return None,
        })
    }

    /// The name the progress window shows: the file's stem, or the new session's name.
    pub fn display_name(&self) -> String {
        match self {
            OpenRequest::Open { path } | OpenRequest::Revert { path } | OpenRequest::DawProject { path } => {
                Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone())
            }
            OpenRequest::Template { name, .. } => name.clone(),
            OpenRequest::Fresh { session } => session.name.clone(),
        }
    }
}

/// A session loaded by [`load_session`], ready for [`install_loaded`].
#[derive(Debug)]
pub struct LoadedSession {
    pub session: Session,
    /// The file the session is bound to afterwards (`Engine::path`).
    pub path: Option<String>,
    /// What could not be found or decoded (the "missing media" report).
    pub missing: Vec<String>,
    /// Which command's result shape `install_loaded` returns.
    pub request: OpenRequestKind,
    /// A DAWproject import's report and the plugin automation still to resolve.
    pub import: Option<Box<DawImport>>,
}

/// What a DAWproject import carries to the install, besides the session.
#[derive(Debug)]
pub struct DawImport {
    pub report: crate::dawproject::Report,
    pub pending: Vec<crate::dawproject::PendingAuto>,
}

/// The shape of a request, kept with its result (the session itself has moved on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenRequestKind {
    Open,
    Revert,
    Template,
    Fresh,
    DawProject,
}

/// The slow, thread-safe half: reads the file and its media, decodes the audio and builds the
/// waveforms, reporting each step to `progress` (Reading, then Audio and Waveforms with counts
/// and file names). Touches no engine. `Fresh` requests report Reading alone.
pub fn load_session(req: OpenRequest, progress: &mut dyn FnMut(OpenProgress)) -> Result<LoadedSession> {
    progress(OpenProgress::new(OpenPhase::Reading, 0, 0, req.display_name()));
    match req {
        OpenRequest::Open { path } => {
            let (session, missing) = crate::io::load_session_file_reporting(&path, Path::new(&path), false, progress)?;
            Ok(LoadedSession { session, path: Some(path), missing, request: OpenRequestKind::Open, import: None })
        }
        OpenRequest::Revert { path } => {
            let (session, missing) = crate::io::load_session_file_reporting(&path, Path::new(&path), false, progress)?;
            Ok(LoadedSession { session, path: Some(path), missing, request: OpenRequestKind::Revert, import: None })
        }
        OpenRequest::Template { path, name } => {
            let (mut session, missing) = crate::io::load_session_file_reporting(&path, Path::new(&path), true, progress)?;
            session.name = name;
            session.folder = None;
            Ok(LoadedSession { session, path: None, missing, request: OpenRequestKind::Template, import: None })
        }
        OpenRequest::Fresh { session } => {
            Ok(LoadedSession { session: *session, path: None, missing: Vec::new(), request: OpenRequestKind::Fresh, import: None })
        }
        OpenRequest::DawProject { path } => {
            let imported = crate::dawproject::import_path(&path, crate::dawproject::import::installed_from_caches(), progress)
                .map_err(crate::EngineError::Io)?;
            let crate::dawproject::import::Imported { session, report, pending } = imported;
            Ok(LoadedSession {
                session,
                path: None,
                missing: Vec::new(),
                request: OpenRequestKind::DawProject,
                import: Some(Box::new(DawImport { report, pending })),
            })
        }
    }
}

/// The other half, on the engine's thread: the loaded session replaces the document (history
/// cleared, transport reset), the missing media is reported, and the command's result is
/// returned as `session.open` / `session.new` / `session.close` / `session.revert` return it.
pub fn install_loaded(e: &mut Engine, loaded: LoadedSession) -> Value {
    let LoadedSession { mut session, path, missing, request, import } = loaded;
    // A DAWproject import: plugin automation whose ranges are known now (the host made the
    // plugins first) is written, and the report goes into the Notes tab.
    let report = import.map(|imp| {
        let DawImport { mut report, mut pending } = *imp;
        crate::dawproject::finish_import(&mut session, &mut report, &mut pending);
        report
    });
    e.replace_session(session);
    e.path = path.clone();
    if report.is_some() {
        // A new, unsaved session: closing it asks to save.
        e.mark_unsaved();
    }
    for m in &missing {
        e.message(format!("missing media: {m}"));
    }
    let s = e.session();
    match request {
        OpenRequestKind::Open => json!({"name": s.name, "tracks": s.tracks.len(), "missing": missing}),
        OpenRequestKind::Revert => json!({}),
        OpenRequestKind::Template => json!({"name": s.name, "sample_rate": s.sample_rate.hz(), "tracks": s.tracks.len(), "missing": missing}),
        OpenRequestKind::Fresh => json!({"name": s.name, "sample_rate": s.sample_rate.hz()}),
        OpenRequestKind::DawProject => {
            let r = report.unwrap_or_default();
            json!({"name": s.name, "sample_rate": s.sample_rate.hz(), "tracks": s.tracks.len(), "report": r, "report_text": r.to_text()})
        }
    }
}

/// The sample rate the session file at `path` is saved at (`None` when it cannot be read): a
/// host opening its audio device before the window's open can open it at the session's rate.
pub fn sample_rate_of(path: &str) -> Option<SampleRate> {
    let text = crate::io::read_text(path).ok()?;
    Session::from_json(&text).ok().map(|s| s.sample_rate)
}

/// Both halves in one go, for the commands themselves (headless hosts, the control channel).
pub fn open_now(e: &mut Engine, req: OpenRequest) -> Result<Value> {
    let loaded = load_session(req, &mut |_| {})?;
    Ok(install_loaded(e, loaded))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_is_weighted_by_phase_and_the_weights_sum_to_one() {
        let total: f32 = OpenPhase::ALL.iter().map(|p| p.weight()).sum();
        assert!((total - 1.0).abs() < 1e-6);
        assert_eq!(OpenProgress::new(OpenPhase::Reading, 0, 0, "").fraction(), 0.0);
        let half_audio = OpenProgress::new(OpenPhase::Audio, 2, 4, "a.wav").fraction();
        assert!((half_audio - 0.175).abs() < 1e-6, "{half_audio}");
        let plugins_start = OpenProgress::new(OpenPhase::Plugins, 0, 3, "").fraction();
        assert!((plugins_start - 0.40).abs() < 1e-6, "{plugins_start}");
        assert!((OpenProgress::new(OpenPhase::Finishing, 0, 0, "").fraction() - 0.95).abs() < 1e-6);
        assert_eq!(OpenProgress::new(OpenPhase::Audio, 2, 7, "").phase_line(), "Loading audio (2 of 7)");
        assert_eq!(OpenProgress::new(OpenPhase::Finishing, 0, 0, "").phase_line(), "Finishing");
    }

    #[test]
    fn loading_reports_reading_audio_and_waveforms_in_order_with_file_names() {
        let dir = std::env::temp_dir().join(format!("soundcraft-open-job-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = crate::demo::demo_engine();
        let path = dir.join("Demo.fdawm").to_string_lossy().into_owned();
        e.execute("session.save_as", &json!({"path": path})).unwrap();
        let files = e.session().sources.len();
        assert!(files >= 2, "{files}");

        let mut seen = Vec::new();
        let req = OpenRequest::from_command(&Engine::default(), "session.open", &json!({"path": path})).unwrap().unwrap();
        assert_eq!(req.display_name(), "Demo");
        let loaded = load_session(req, &mut |p| seen.push(p)).unwrap();
        assert_eq!(seen[0].phase, OpenPhase::Reading);
        let audio: Vec<&OpenProgress> = seen.iter().filter(|p| p.phase == OpenPhase::Audio).collect();
        let waves: Vec<&OpenProgress> = seen.iter().filter(|p| p.phase == OpenPhase::Waveforms).collect();
        assert_eq!(audio.len(), files, "one report per file");
        assert!(audio.iter().all(|p| p.total == files && p.item.ends_with(".wav")), "{audio:?}");
        assert!(audio.windows(2).all(|w| w[0].done < w[1].done));
        assert_eq!(waves.len(), files + 1, "one per file, then the phase complete");
        assert!(waves.last().is_some_and(|p| p.done == p.total));
        // Phases never go backwards.
        let order: Vec<usize> = seen.iter().map(|p| OpenPhase::ALL.iter().position(|x| *x == p.phase).unwrap()).collect();
        assert!(order.is_sorted(), "{order:?}");
        assert!(loaded.missing.is_empty());
        assert_eq!(loaded.session.pool.len(), files);
        assert!(loaded.session.pool.get(loaded.session.sources[0].id).is_some_and(|a| !a.peaks.is_empty()), "waveforms built");

        // Installing it is what `session.open` does.
        let mut target = Engine::default();
        let v = install_loaded(&mut target, loaded);
        assert_eq!(v["name"], "Demo");
        assert_eq!(target.path.as_deref(), Some(path.as_str()));
        assert!(!target.is_dirty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fresh_and_revert_requests_come_from_the_engine() {
        let e = Engine::default();
        assert!(OpenRequest::from_command(&e, "session.revert", &json!({})).unwrap().is_err(), "unsaved: nothing to revert to");
        assert!(OpenRequest::from_command(&e, "mix.volume", &json!({})).is_none());
        let OpenRequest::Fresh { session } =
            OpenRequest::from_command(&e, "session.new", &json!({"name": "X", "sample_rate": 44100})).unwrap().unwrap()
        else {
            panic!()
        };
        assert_eq!((session.name.as_str(), session.sample_rate.hz()), ("X", 44_100));
        assert!(matches!(OpenRequest::from_command(&e, "session.close", &json!({})).unwrap().unwrap(), OpenRequest::Fresh { .. }));
        let saved = Engine { path: Some("here.fdawm".into()), ..Engine::default() };
        assert_eq!(
            OpenRequest::from_command(&saved, "session.revert", &json!({})).unwrap().unwrap(),
            OpenRequest::Revert { path: "here.fdawm".into() }
        );
    }
}
