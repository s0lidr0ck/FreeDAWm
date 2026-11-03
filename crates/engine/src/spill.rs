//! Generated audio goes to disk when it is made (spec 2026-11-02, disk streaming): a source held
//! only in memory and not written yet (stems, Clean Vocal, tuning renders, freeze, Process,
//! consolidate, Split into Files, imports) is written as a 32-bit float WAV right after the step
//! that made it, then plays from disk like any file, so neither the session nor its undo history
//! keeps it in memory.
//!
//! Where: a saved session's project folder, in `Audio Files/` (imports and recordings) or a
//! subfolder by kind ([`SUBFOLDERS`]); a session never saved, the same layout under its autosave
//! folder (`<autosave root>/<session name>/Audio Files/…`, where its takes go too), copied into the
//! project at the first save. Without either (a headless engine never saved) audio stays in memory.
//!
//! Generated files no clip or undo step uses any more are deleted when the session is saved
//! ([`clean_unused`]): only files inside the project's generated subfolders, never recordings or
//! imports.

use crate::Engine;
use soundcraft_model::{Session, SourceAudio, SourceId};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The subfolders of `Audio Files/` generated audio goes in (spec, goal 4).
pub const STEMS: &str = "Stems";
pub const CLEAN_VOCAL: &str = "Clean Vocal";
pub const TUNING: &str = "Tuning";
pub const FREEZE: &str = "Freeze";
pub const PROCESSED: &str = "Processed";
pub const SPLIT: &str = "Split";
pub const SUBFOLDERS: [&str; 6] = [STEMS, CLEAN_VOCAL, TUNING, FREEZE, PROCESSED, SPLIT];

/// Sources smaller than this many samples (all channels) stay in memory: not worth a file.
pub const MIN_SAMPLES: usize = 1 << 16;

/// The subfolder for audio made by the step labelled `label` (an undo label or command label);
/// `None`: `Audio Files/` itself (imports, recordings, sampler loads).
pub fn subfolder_for(label: &str) -> Option<&'static str> {
    let l = label.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| l.contains(w));
    if has(&["stem", "karaoke", "drum"]) {
        Some(STEMS)
    } else if has(&["clean"]) {
        Some(CLEAN_VOCAL)
    } else if has(&["tun", "vocal", "pitch"]) {
        Some(TUNING)
    } else if has(&["split into files"]) {
        Some(SPLIT)
    } else if has(&["import", "record", "sampler", "sample", "open", "paste", "dawproject", "audio file"]) {
        None
    } else if has(&["freeze", "commit"]) {
        Some(FREEZE)
    } else {
        Some(PROCESSED)
    }
}

/// The generated subfolder a stored source path is in (`Audio Files/Stems/x.wav` → `Stems`; an
/// absolute path under an `Audio Files` folder too).
pub fn subfolder_of(path: &str) -> Option<&'static str> {
    let p = Path::new(path);
    let parent = p.parent()?;
    let name = parent.file_name()?.to_str()?;
    let above = parent.parent()?.file_name()?.to_str()?;
    (above == "Audio Files").then(|| SUBFOLDERS.iter().copied().find(|s| *s == name)).flatten()
}

/// Sources a sample-playing instrument plays: kept in memory (it reads them whole).
fn sampled(s: &Session) -> HashSet<SourceId> {
    s.tracks.iter().flat_map(|t| t.mixer.inserts.iter().chain(t.instrument.as_ref())).filter_map(|i| i.sample).collect()
}

/// A stored source path, relative ones taken from `base`.
fn resolve(stored: &str, base: &Path) -> PathBuf {
    let p = Path::new(stored);
    if p.is_absolute() { p.to_path_buf() } else { base.join(p) }
}

/// The source paths a session file's text names, `None` when it isn't one that reads.
fn source_paths(text: &str) -> Option<Vec<String>> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let sources = match v.get("sources") {
        None | Some(serde_json::Value::Null) => return Some(Vec::new()),
        Some(s) => s.as_array()?,
    };
    Some(sources.iter().filter_map(|s| s.get("path")?.as_str().map(str::to_string)).collect())
}

/// Every audio file named by the session files in `project` (its session files, `Backups/`) and
/// the autosaves under `autosave_root`; `None` when one of them can't be read.
fn named_on_disk(project: &Path, autosave_root: Option<&Path>) -> Option<Vec<PathBuf>> {
    let mut out = Vec::new();
    let is_session = |f: &Path| {
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
        soundcraft_model::SESSION_EXTENSIONS.iter().any(|x| name.ends_with(&format!(".{x}"))) || name.ends_with(".tmp")
    };
    // A folder that may not be read could hide a session: then nothing is deleted.
    let backups = project.join("Backups");
    crate::file_access::check_read(project).ok()?;
    crate::file_access::check_read(&backups).ok()?;
    let mut files: Vec<PathBuf> = crate::io::list_dir(project).into_iter().filter(|f| is_session(f)).collect();
    // Every backup, whatever it is called.
    files.extend(crate::io::list_dir(&backups));
    for f in files {
        if !crate::io::file_found(&f) {
            continue;
        }
        let paths = source_paths(&crate::io::read_text(&f).ok()?)?;
        out.extend(paths.iter().map(|p| resolve(p, project)));
    }
    // Crash-recovery copies: their paths are absolute, or relative to the session they came from.
    if let Some(root) = autosave_root {
        crate::file_access::check_read(root).ok()?;
    }
    for dir in autosave_root.map(crate::io::list_dir).unwrap_or_default() {
        for name in ["session.fdawm", "session.scraft"] {
            let f = dir.join(name);
            let Ok(text) = crate::io::read_text(&f) else { continue };
            let origin = crate::io::read_text(dir.join("origin")).ok().map(PathBuf::from);
            let base = origin.as_deref().and_then(Path::parent).unwrap_or(project).to_path_buf();
            out.extend(source_paths(&text)?.iter().map(|p| resolve(p, &base)));
        }
    }
    Some(out)
}

/// Writes the generated audio of the step labelled `label` to disk (see the module docs) and
/// swaps each source for its file. Write errors leave the audio in memory (saved later as before).
pub(crate) fn spill(e: &mut Engine, label: &str) {
    if !soundcraft_model::disk::streaming_enabled() {
        return;
    }
    let s = e.session();
    let todo: Vec<SourceId> = {
        let keep = sampled(s);
        s.sources
            .iter()
            .filter(|src| src.unsaved && !keep.contains(&src.id))
            .filter(|src| s.pool.get(src.id).is_some_and(|a| a.in_memory().is_some() && a.frames().saturating_mul(a.num_channels()) >= MIN_SAMPLES))
            .map(|src| src.id)
            .collect()
    };
    if todo.is_empty() {
        return;
    }
    // A saved session writes into its project; one never saved under its autosave folder.
    let (base, in_project) = match (e.path.as_ref(), e.session().folder.as_ref()) {
        (Some(_), Some(folder)) => (folder.clone(), true),
        _ => match &e.unsaved_audio_root {
            Some(root) => (root.join(crate::io::sanitize_name(&e.session().name)), false),
            None => return,
        },
    };
    let sub = subfolder_for(label);
    let dir = sub.map_or_else(|| base.join("Audio Files"), |sub| base.join("Audio Files").join(sub));
    let rate = e.session().sample_rate.hz();
    let mut done: Vec<(SourceId, Arc<SourceAudio>, String)> = Vec::new();
    let mut written: Vec<(*const SourceAudio, Arc<SourceAudio>, String)> = Vec::new();
    for id in todo {
        let s = e.session();
        let (Some(src), Some(audio)) = (s.sources.iter().find(|x| x.id == id), s.pool.get(id)) else { continue };
        // Sources sharing one audio share one file.
        if let Some((_, a, path)) = written.iter().find(|(p, _, _)| *p == Arc::as_ptr(audio)) {
            done.push((id, Arc::clone(a), path.clone()));
            continue;
        }
        let Some(buf) = audio.in_memory() else { continue };
        let Some((file, name)) = crate::io::write_wav_new(&dir, &crate::io::sanitize_name(&src.name), buf) else { continue };
        let Some(f) = crate::io::streamable(&file, rate) else {
            let _ = crate::io::remove_path(&file);
            continue;
        };
        let streamed = match SourceAudio::from_disk(f, audio.peaks.clone()) {
            Ok(a) => Arc::new(a),
            Err(_) => continue,
        };
        let stored = if in_project {
            sub.map_or_else(|| format!("Audio Files/{name}"), |sub| format!("Audio Files/{sub}/{name}"))
        } else {
            file.to_string_lossy().into_owned()
        };
        // The waveform cache gets it now (a saved project), so a reopen reads no audio for it.
        if in_project {
            crate::peak_cache::store(&base, &file, rate, &streamed.peaks);
        }
        written.push((Arc::as_ptr(audio), Arc::clone(&streamed), stored.clone()));
        done.push((id, streamed, stored));
    }
    if done.is_empty() {
        return;
    }
    let s = e.session_mut();
    for (id, audio, stored) in done {
        s.pool.insert(id, audio);
        if let Some(src) = s.sources.iter_mut().find(|x| x.id == id) {
            src.path = stored;
            src.format = soundcraft_audio_io::FileFormat::Wav;
            // In the project it is saved; under the autosave folder it is copied in at the first save.
            src.unsaved = !in_project;
        }
    }
}

/// Deletes the generated files in `project`'s generated subfolders that nothing names: not a
/// source of `keep` (the open session and its undo history), not any session file in the
/// project folder (other versions, Save As copies), not any backup in `Backups/`, not any
/// autosave under `autosave_root`. When one of those can't be read, nothing is deleted (when
/// unsure, keep). Returns how many were deleted.
pub fn clean_unused(project: &Path, keep: &[&Session], autosave_root: Option<&Path>) -> usize {
    let audio = project.join("Audio Files");
    let Some(on_disk) = named_on_disk(project, autosave_root) else { return 0 };
    let used: HashSet<PathBuf> =
        keep.iter().flat_map(|s| s.sources.iter()).map(|src| resolve(&src.path, project)).chain(on_disk).map(|p| crate::io::file_key(&p)).collect();
    let mut gone = 0;
    for sub in SUBFOLDERS {
        let dir = audio.join(sub);
        for f in crate::io::list_dir(&dir) {
            let is_wav = f.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("wav"));
            if !is_wav || used.contains(&crate::io::file_key(&f)) {
                continue;
            }
            if crate::io::remove_path(&f).is_ok() {
                gone += 1;
                // Its waveform cache entry goes with it.
                if let Some(cache) = crate::peak_cache::path_for(project, &f) {
                    let _ = crate::io::remove_path(cache);
                }
            }
        }
    }
    gone
}
