//! Auditioning: a clip from the Clip List plays through the preview voice (again: stops), a file
//! is read off the window's thread, and the Clip List finds the files nothing uses.

use serde_json::json;
use soundcraft_ui_egui::{Services, SoundApp, audition, panels};

fn app() -> SoundApp {
    SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default())
}

#[test]
fn a_clip_auditions_and_a_second_click_stops_it() {
    let mut app = app();
    let clip = app.engine.session().tracks.iter().flat_map(|t| t.clips()).find(|c| c.is_audio()).unwrap().id;
    audition::clip(&mut app, clip);
    assert_eq!(app.audition.playing.as_deref(), Some(format!("clip:{}", clip.0).as_str()));
    audition::clip(&mut app, clip);
    assert_eq!(app.audition.playing, None);
}

#[test]
fn a_file_is_read_on_a_worker_and_then_plays() {
    let dir = std::env::temp_dir().join(format!("freedawm-audition-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tone.wav");
    let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 4800]] };
    std::fs::write(&path, soundcraft_audio_io::encode(&buf, &Default::default()).unwrap()).unwrap();
    let mut app = app();
    let p = path.to_string_lossy().into_owned();
    audition::file(&mut app, &p);
    assert!(audition::loading(&app));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while audition::loading(&app) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
        audition::poll(&mut app);
    }
    assert_eq!(app.audition.playing.as_deref(), Some(p.as_str()), "{}", app.ui.status);
    audition::file(&mut app, &p);
    assert_eq!(app.audition.playing, None, "a second click stops it");
    // A file that is not audio says so instead.
    let bad = dir.join("bad.wav");
    std::fs::write(&bad, b"not audio").unwrap();
    audition::file(&mut app, &bad.to_string_lossy());
    while audition::loading(&app) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(5));
        audition::poll(&mut app);
    }
    assert!(app.ui.status.starts_with("Cannot audition"), "{}", app.ui.status);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_clip_list_finds_files_nothing_plays() {
    let mut app = app();
    assert!(panels::unused_sources(app.engine.session()).is_empty(), "the demo uses every file");
    let t = app.engine.session().tracks.iter().find(|t| t.clips().iter().any(|c| c.is_audio())).unwrap().id;
    app.run("edit.select", json!({"tracks": [t.0], "start": 0, "end": app.engine.session().content_end()})).unwrap();
    app.run("edit.clear", json!({})).unwrap();
    let unused = panels::unused_sources(app.engine.session());
    assert!(!unused.is_empty(), "the cleared track's files are no longer used");
    let s = app.engine.session();
    assert!(
        unused.iter().all(|id| !s.all_clips().any(|c| matches!(c.content, soundcraft_model::ClipContent::Audio { source, .. } if source == *id)))
    );
}
