//! Generated audio is written to disk when it is made, into `Audio Files/<kind>/` (a saved
//! session) or under the autosave folder (a session never saved), and then plays from its file;
//! undo brings back clips whose files are still there; saving deletes generated files nothing
//! uses any more; a crash before the first save loses none of it.

use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_engine::autosave::{Autosaver, leftovers, recover};
use soundcraft_model::{Clip, TrackId};
use soundcraft_time::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("freedawm-gen-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const SECS: i64 = 3;

/// A stereo track with a 3 s clip of a ramp, selected for Consolidate.
fn engine() -> (Engine, TrackId) {
    let mut e = Engine::default();
    e.execute("track.new", &json!({"count": 1, "format": "stereo"})).unwrap();
    let t = e.session().tracks[0].id;
    let frames = (SECS * 48_000) as usize;
    let ramp: Vec<f32> = (0..frames).map(|i| (i % 1000) as f32 / 2000.0).collect();
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![ramp.clone(), ramp] };
    let src = soundcraft_engine::io::add_source(e.session_mut(), "ramp", buf, None, FileFormat::Wav);
    let id = e.session_mut().new_clip_id();
    soundcraft_engine::edit::place_clip(e.session_mut(), t, Clip::audio(id, "ramp", src, 0, 0, frames as i64));
    e.session_mut().edit.selected_tracks = vec![t];
    e.session_mut().edit.selection = Range::new(0, (SECS + 1) * 48_000);
    (e, t)
}

/// The source of the only clip on `t`: (stored path, unsaved, plays from disk, first samples).
fn only_source(e: &Engine, t: TrackId) -> (String, bool, bool, Vec<f32>) {
    let s = e.session();
    let c = &s.track(t).unwrap().clips()[0];
    let src = s.sources.iter().find(|x| Some(x.id) == c.source()).unwrap();
    let a = s.pool.get(src.id).unwrap();
    (src.path.clone(), src.unsaved, a.disk_file().is_some(), a.window(0, 8)[0].clone())
}

fn wavs(d: &Path) -> usize {
    std::fs::read_dir(d).map_or(0, |r| r.flatten().filter(|f| f.path().extension().is_some_and(|x| x == "wav")).count())
}

#[test]
fn a_saved_sessions_generated_audio_is_in_its_subfolder_before_save_and_cleaned_when_unused() {
    let d = dir("saved");
    let (mut e, t) = engine();
    let path = d.join("Song").join("Song.fdawm");
    e.execute("session.save_as", &json!({"path": path.to_string_lossy()})).unwrap();
    // The imported audio was written at the save and now plays from its file.
    let (p, unsaved, from_disk, _) = only_source(&e, t);
    assert!(p.starts_with("Audio Files/") && !unsaved && from_disk, "{p}");
    e.execute("edit.consolidate", &json!({})).unwrap();
    let (p, unsaved, from_disk, first) = only_source(&e, t);
    assert!(p.starts_with("Audio Files/Processed/"), "{p}");
    assert!(!unsaved && from_disk, "written when made, played from disk");
    assert!(d.join("Song").join(&p).is_file(), "on disk before any save");
    assert_eq!(first, (0..8).map(|i| (i % 1000) as f32 / 2000.0).collect::<Vec<_>>());
    let processed = d.join("Song").join("Audio Files").join("Processed");
    // Undo brings back the clip of the original file; the generated file stays for Redo.
    e.execute("edit.undo", &json!({})).unwrap();
    assert!(only_source(&e, t).0.starts_with("Audio Files/ramp"));
    e.execute("session.save", &json!({})).unwrap();
    assert_eq!(wavs(&processed), 1, "a redo step still uses it");
    e.execute("edit.redo", &json!({})).unwrap();
    assert!(only_source(&e, t).0.starts_with("Audio Files/Processed/"));
    // Undone and then replaced by another edit: nothing uses it; the next save deletes it.
    e.execute("edit.undo", &json!({})).unwrap();
    e.execute("track.new", &json!({"count": 1})).unwrap();
    e.execute("session.save", &json!({})).unwrap();
    assert_eq!(wavs(&processed), 0, "unused generated audio is cleaned up at save");
    assert_eq!(wavs(&d.join("Song").join("Audio Files")), 1, "the imported file is never touched");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_never_saved_sessions_generated_audio_survives_a_crash_and_moves_in_at_the_first_save() {
    let d = dir("unsaved");
    let root = d.join("Autosave");
    let (mut e, t) = engine();
    e.unsaved_audio_root = Some(root.clone());
    e.execute("edit.consolidate", &json!({})).unwrap();
    let (p, unsaved, from_disk, first) = only_source(&e, t);
    assert!(Path::new(&p).is_absolute() && Path::new(&p).is_file() && unsaved && from_disk, "{p}");
    assert!(Path::new(&p).starts_with(root.join("Untitled").join("Audio Files").join("Processed")), "{p}");
    // The autosave refers to it rather than copying it; a crash loses nothing.
    let saver = Autosaver::new(root.clone());
    saver.write(e.session_arc(), None, None);
    saver.flush().unwrap();
    drop(saver);
    let left = leftovers(&root);
    assert_eq!(left.len(), 1);
    let mut r = Engine::default();
    assert_eq!(recover(&mut r, &left[0]).unwrap(), Vec::<String>::new());
    let (rp, runsaved, rdisk, rfirst) = only_source(&r, t);
    assert_eq!((rp.as_str(), runsaved, rdisk, &rfirst), (p.as_str(), true, true, &first));
    // The first save copies it into the project's subfolder.
    let path = d.join("Song").join("Song.fdawm");
    r.execute("session.save_as", &json!({"path": path.to_string_lossy()})).unwrap();
    let (sp, sunsaved, _, sfirst) = only_source(&r, t);
    assert!(sp.starts_with("Audio Files/Processed/") && !sunsaved, "{sp}");
    assert_eq!(sfirst, first);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_reopened_session_plays_from_disk_and_renders_as_before() {
    let d = dir("reopen");
    let (mut e, _) = engine();
    let before = soundcraft_mix::render_range(e.session(), Range::new(0, SECS * 48_000), 512);
    let path = d.join("Song").join("Song.fdawm");
    e.execute("session.save_as", &json!({"path": path.to_string_lossy()})).unwrap();
    let mut r = Engine::default();
    r.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
    assert!(r.session().pool.ids().all(|id| r.session().pool.get(id).is_some_and(|a| a.disk_file().is_some())));
    let after = soundcraft_mix::render_range(r.session(), Range::new(0, SECS * 48_000), 512);
    let bits = |x: &Vec<Vec<f32>>| x.iter().map(|c| c.iter().map(|v| v.to_bits()).collect::<Vec<_>>()).collect::<Vec<_>>();
    assert_eq!(bits(&before), bits(&after));
    // Undo history points at files: nothing decoded is kept alive by it.
    assert!(Arc::strong_count(r.session().pool.get(r.session().sources[0].id).unwrap()) >= 1);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn cleanup_keeps_what_a_backup_or_another_session_file_in_the_folder_names() {
    let d = dir("backups");
    let (mut e, t) = engine();
    let song = d.join("Song");
    let path = song.join("Song.fdawm");
    e.execute("session.save_as", &json!({"path": path.to_string_lossy()})).unwrap();
    e.execute("edit.consolidate", &json!({})).unwrap();
    let (rel, ..) = only_source(&e, t);
    let generated = song.join(&rel);
    e.execute("session.save", &json!({})).unwrap();
    assert!(generated.is_file());
    // Reopened (no history), the generated clip and its source go without an undo step.
    let mut r = Engine::default();
    r.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
    let drop_generated = |r: &mut Engine| {
        let s = r.session_mut();
        let id = s.sources.iter().find(|x| x.path == rel).unwrap().id;
        for tr in &mut s.tracks {
            for p in &mut tr.playlists {
                p.clips.retain(|c| c.source() != Some(id));
            }
        }
        s.sources.retain(|x| x.id != id);
        s.pool.remove(id);
    };
    drop_generated(&mut r);
    // The backup this save makes of the version before still names it: kept.
    r.execute("session.save", &json!({})).unwrap();
    assert!(generated.is_file(), "a backup names it");
    // No backups, but another session file in the folder names it: kept.
    let backups = song.join("Backups");
    let name = std::path::Path::new(&rel).file_name().unwrap().to_string_lossy().into_owned();
    let older = std::fs::read_dir(&backups)
        .unwrap()
        .flatten()
        .map(|f| f.path())
        .find(|f| std::fs::read_to_string(f).unwrap().contains(&name))
        .expect("the backup naming it");
    std::fs::copy(&older, song.join("Song v1.fdawm")).unwrap();
    std::fs::remove_dir_all(&backups).unwrap();
    r.execute("session.save", &json!({})).unwrap();
    assert!(generated.is_file(), "another session file names it");
    // An unreadable session file in the folder: nothing is deleted.
    std::fs::remove_file(song.join("Song v1.fdawm")).unwrap();
    std::fs::write(song.join("Broken.fdawm"), "not json").unwrap();
    r.execute("session.save", &json!({})).unwrap();
    assert!(generated.is_file(), "when unsure, keep");
    // Nothing names it any more: deleted at the next save.
    std::fs::remove_file(song.join("Broken.fdawm")).unwrap();
    let _ = std::fs::remove_dir_all(&backups);
    r.execute("session.save", &json!({})).unwrap();
    assert!(!generated.is_file(), "unused generated audio is cleaned up");
    let _ = std::fs::remove_dir_all(&d);
}
