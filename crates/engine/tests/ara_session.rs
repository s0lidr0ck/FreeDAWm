//! ARA stage one against the session: imported files of several formats reach the test ARA
//! plug-in sample for sample (32- and 64-bit reads), and the ARA document follows moves, trims,
//! deletes, undo and redo through the engine's own commands.

use serde_json::json;
use soundcraft_ara_host::test_plugin::{Behaviour, checksum, factory_ptr, report};
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_engine::ara::{AraHost, Factory};
use soundcraft_model::{ClipAra, ClipId, SourceId};

const PLUGIN: &str = "test:good";

fn host() -> AraHost {
    AraHost::new(Box::new(|id| {
        if id == PLUGIN { Ok(Factory { ptr: factory_ptr(Behaviour::Good), keep: Box::new(()) }) } else { Err(format!("{id} is not installed")) }
    }))
}

fn signal(frames: usize, seed: f32) -> AudioBuffer {
    let ch = |c: f32| (0..frames).map(|i| (i as f32 * 0.0123 + seed + c).sin() * 0.6 + ((i * 7) % 13) as f32 * 0.01).collect::<Vec<f32>>();
    AudioBuffer { sample_rate: 48_000, channels: vec![ch(0.0), ch(1.3)] }
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("freedawm-ara-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Writes `buf` as a file, imports it on a new track and gives its clip to the ARA plug-in.
fn import(e: &mut Engine, dir: &std::path::Path, file: &str, format: FileFormat, depth: BitDepth, buf: &AudioBuffer) -> (ClipId, SourceId, Vec<u8>) {
    let bytes = soundcraft_audio_io::encode(buf, &EncodeOptions { format, bit_depth: depth, ..EncodeOptions::default() }).unwrap();
    let path = dir.join(file);
    std::fs::write(&path, &bytes).unwrap();
    e.execute("file.import_audio", &json!({"path": path.to_string_lossy()})).unwrap();
    let t = e.session().tracks.last().unwrap();
    let c = t.clips().first().unwrap();
    let (id, src) = (c.id, c.source().unwrap());
    e.session_mut().find_clip_mut(id).unwrap().ara = Some(ClipAra { plugin: PLUGIN.into(), name: "Test ARA".into() });
    (id, src, bytes)
}

fn decoded(bytes: &[u8], ext: &str) -> Vec<Vec<f64>> {
    let (_, buf) = soundcraft_audio_io::decode(bytes, Some(ext)).unwrap();
    buf.channels.iter().map(|c| c.iter().map(|x| f64::from(*x)).collect()).collect()
}

#[test]
fn files_of_several_formats_reach_the_plug_in_sample_for_sample() {
    let dir = temp_dir("formats");
    let mut e = Engine::default();
    e.session_mut().name = "ara formats".into();
    let buf = signal(20_000, 0.4);
    let kinds = [
        ("a.wav", FileFormat::Wav, BitDepth::Int16, "wav"),
        ("b.wav", FileFormat::Wav, BitDepth::Int24, "wav"),
        ("c.wav", FileFormat::Wav, BitDepth::Float32, "wav"),
        ("d.flac", FileFormat::Flac, BitDepth::Int24, "flac"),
        ("e.aiff", FileFormat::Aiff, BitDepth::Int16, "aiff"),
    ];
    let files: Vec<(ClipId, SourceId, Vec<u8>, &str)> = kinds
        .iter()
        .map(|(name, f, d, ext)| {
            let (c, s, b) = import(&mut e, &dir, name, *f, *d, &buf);
            (c, s, b, *ext)
        })
        .collect();
    let mut h = host();
    assert!(h.sync(&e));
    assert_eq!(h.counts(PLUGIN), Some((5, 5, 5, 5, 0)));
    for (clip, src, bytes, ext) in &files {
        let want = checksum(&decoded(bytes, ext));
        let notes = h.notes(PLUGIN, *src).expect("the plug-in read the file");
        assert_eq!(notes.len(), 2, "{ext}");
        assert_eq!(notes[0].start, (want >> 11) as f64, "{ext}: the 32-bit read is the file's samples");
        assert_eq!(notes[1].start, (want >> 11) as f64, "{ext}: the 64-bit read too");
        assert_eq!((notes[0].pitch, notes[0].duration), (2, 20_000.0), "{ext}");
        let status = h.status(e.session().find_clip(*clip).unwrap().1).unwrap();
        assert!(status.contains("FreeDAWm Test ARA") && status.contains("2 notes"), "{status}");
    }
    let r = report("ara formats").unwrap();
    assert!(r.violations.is_empty(), "{:?}", r.violations);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_document_follows_edits_undo_and_redo() {
    let dir = temp_dir("edits");
    let mut e = Engine::default();
    e.session_mut().name = "ara edits".into();
    let buf = signal(96_000, 1.1);
    let (a, _, _) = import(&mut e, &dir, "vox.wav", FileFormat::Wav, BitDepth::Int24, &buf);
    let (b, _, _) = import(&mut e, &dir, "gtr.wav", FileFormat::Wav, BitDepth::Int16, &buf);
    let mut h = host();
    h.sync(&e);
    assert_eq!(h.counts(PLUGIN), Some((2, 2, 2, 2, 0)));
    let calls = |name: &str| report(name).unwrap().calls.len();
    let before = calls("ara edits");
    // Nothing changed: no ARA call but the periodic notifyModelUpdates.
    assert!(!h.sync(&e));
    assert_eq!(report("ara edits").unwrap().calls[before..].iter().filter(|c| *c != "notifyModelUpdates").count(), 0);
    // Move, trim: the playback region is updated.
    e.execute("edit.move_clips", &json!({"clips": [a.0], "to": 48_000})).unwrap();
    assert!(h.sync(&e));
    e.execute("edit.trim_clip", &json!({"clip": a.0, "start": 60_000, "alone": true})).unwrap();
    assert!(h.sync(&e));
    assert!(report("ara edits").unwrap().calls.iter().filter(|c| *c == "updatePlaybackRegionProperties").count() >= 2);
    assert_eq!(h.counts(PLUGIN), Some((2, 2, 2, 2, 0)));
    // Delete the guitar clip: its region goes, its modification and file are kept for undo.
    let t = e.session().find_clip(b).unwrap().0;
    e.execute("edit.clear", &json!({"tracks": [t.0], "start": 0, "end": e.session().content_end()})).unwrap();
    h.sync(&e);
    assert_eq!(h.counts(PLUGIN), Some((1, 1, 1, 1, 1)));
    // Undo the delete: it comes back, reactivated.
    e.execute("edit.undo", &json!({})).unwrap();
    h.sync(&e);
    assert_eq!(h.counts(PLUGIN), Some((2, 2, 2, 2, 0)));
    // Undo the trim and the move, then redo all three.
    e.execute("edit.undo", &json!({})).unwrap();
    e.execute("edit.undo", &json!({})).unwrap();
    h.sync(&e);
    e.execute("edit.redo", &json!({})).unwrap();
    e.execute("edit.redo", &json!({})).unwrap();
    e.execute("edit.redo", &json!({})).unwrap();
    h.sync(&e);
    assert_eq!(h.counts(PLUGIN), Some((1, 1, 1, 1, 1)));
    let r = report("ara edits").unwrap();
    assert!(r.violations.is_empty(), "{:?}", r.violations);
    assert_eq!(r.live, (1, 1, 1, 1));
    // Taking the last clip back from the plug-in closes its document.
    e.session_mut().find_clip_mut(a).unwrap().ara = None;
    h.sync(&e);
    assert!(h.documents().is_empty());
    let r = report("ara edits").unwrap();
    assert!(r.destroyed && r.violations.is_empty(), "{:?}", r.violations);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_plug_in_or_an_unsupported_clip_says_why() {
    let dir = temp_dir("missing");
    let mut e = Engine::default();
    let buf = signal(10_000, 0.0);
    let (a, _, _) = import(&mut e, &dir, "x.wav", FileFormat::Wav, BitDepth::Int16, &buf);
    let (b, _, _) = import(&mut e, &dir, "y.wav", FileFormat::Wav, BitDepth::Int16, &buf);
    e.session_mut().find_clip_mut(a).unwrap().ara = Some(ClipAra { plugin: "vst3:gone".into(), name: "Gone".into() });
    e.session_mut().find_clip_mut(b).unwrap().reversed = true;
    let mut h = host();
    h.sync(&e);
    let s = h.status(e.session().find_clip(a).unwrap().1).unwrap();
    assert!(s.starts_with("Gone: cannot be used") && s.contains("not installed"), "{s}");
    let s = h.status(e.session().find_clip(b).unwrap().1).unwrap();
    assert!(s.contains("reversed"), "{s}");
    assert!(h.summary().iter().any(|l| l.contains("vst3:gone")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_tempo_map_reaches_the_plug_in() {
    let dir = temp_dir("tempo");
    let mut e = Engine::default();
    e.session_mut().name = "ara tempo".into();
    e.session_mut().tempo.set_tempo(3840, 90.0).unwrap();
    import(&mut e, &dir, "t.wav", FileFormat::Wav, BitDepth::Int16, &signal(4_000, 0.0));
    let mut h = host();
    h.sync(&e);
    assert_eq!(report("ara tempo").unwrap().tempo_entries, Some(3), "quarter 0, the change at bar 2, one past it");
    let _ = std::fs::remove_dir_all(&dir);
}
