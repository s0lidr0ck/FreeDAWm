//! Waveform overviews of sources played from disk: only their coarse levels are held, the fine
//! ones read from the project's `Waveforms` cache as they are drawn. Paged, they answer every
//! drawing query as the whole overview does once their pages are in; an old cache file is
//! completed; a cache deleted or damaged while open is made again; without a project nothing
//! changes. The page cache is process-wide, so the tests here take turns.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use soundcraft_audio_io::peaks::Peaks;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat, encode};
use soundcraft_engine::{Engine, peak_pages};
use soundcraft_model::{ChannelFormat, Clip, ClipId, Session, Source, SourceAudio, SourceId, TrackKind, disk};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

static TURN: Mutex<()> = Mutex::new(());

fn turn() -> std::sync::MutexGuard<'static, ()> {
    TURN.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("freedawm-overviews-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("Audio Files")).unwrap();
    d
}

const SR: usize = 48_000;

/// A tone with bursts and a slow swell, so every zoom shows a shape.
fn signal(channels: usize, frames: usize) -> AudioBuffer {
    let channels = (0..channels)
        .map(|c| {
            (0..frames)
                .map(|i| {
                    let t = i as f32 / SR as f32;
                    let swell = 0.2 + 0.7 * (t * 0.37 + c as f32).sin().abs();
                    let burst = if (i / 7_919) % 13 == 0 { 0.3 } else { 0.0 };
                    ((t * 440.0 * std::f32::consts::TAU).sin() * swell + burst).clamp(-1.0, 1.0)
                })
                .collect()
        })
        .collect();
    AudioBuffer { sample_rate: SR as u32, channels }
}

/// A saved session in `d` with one clip of a `secs`-second stereo 24-bit file.
fn session(d: &Path, secs: usize) -> (PathBuf, PathBuf) {
    let buf = signal(2, secs * SR + 77);
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int24, dither: false, bwf: None, bitrate_kbps: 0 };
    let file = d.join("Audio Files").join("Long.wav");
    std::fs::write(&file, encode(&buf, &opts).unwrap()).unwrap();
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("A"));
    let id = SourceId(s.alloc());
    s.sources.push(Source {
        id,
        name: "Long".into(),
        path: "Audio Files/Long.wav".into(),
        channels: 2,
        frames: buf.frames() as u64,
        sample_rate: SR as u32,
        format: FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
    });
    let clip = Clip::audio(ClipId(s.alloc()), "c", id, 0, 0, buf.frames() as i64);
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(clip);
    let path = d.join("Song.fdawm");
    std::fs::write(&path, s.to_json().unwrap()).unwrap();
    (path, file)
}

fn open(path: &Path) -> Engine {
    let mut e = Engine::default();
    e.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
    e
}

fn source(e: &Engine) -> Arc<SourceAudio> {
    let s = e.session();
    Arc::clone(s.pool.get(s.sources[0].id).unwrap())
}

/// The whole overviews of `file`, made by reading it through (the "all in memory" overview).
fn whole(file: &Path) -> Vec<Peaks> {
    SourceAudio::from_disk_scanned(disk::DiskFile::open(file).unwrap()).peaks
}

/// What a waveform shows, as the drawing asks: spans and RMS spans at a zoom, asked again until
/// the pages are in (each try never waits).
type Shown = (Vec<Option<(f32, f32)>>, Vec<Option<f32>>);

fn settled(p: &Peaks, window: &[f32], at: usize, start: f64, step: f64, cols: usize) -> (Shown, bool) {
    let done = p.spans_in_checked(window, at, start, step, step, cols).1;
    for _ in 0..50 {
        let (a, d1) = p.spans_in_checked(window, at, start, step, step, cols);
        let (b, d2) = p.rms_spans_in_checked(window, at, start, step, step, cols);
        if d1 && d2 {
            return ((a, b), done);
        }
        peak_pages::wait_idle(Duration::from_secs(10));
    }
    panic!("the pages never came")
}

/// Zooms and positions over a file `frames` long: sample level to the whole file on a screen.
fn views(frames: usize) -> Vec<(f64, f64, usize)> {
    let mut v = Vec::new();
    for step in [3.0, 47.0, 300.0, 1_000.0, 2_500.0, 9_000.0, 40_000.0, 150_000.0] {
        let cols = 1_800usize;
        let span = step * cols as f64;
        for k in 0..3 {
            let start = ((frames as f64 - span).max(0.0) * k as f64 / 2.0).floor() + 0.25;
            v.push((start, step, cols));
        }
    }
    v
}

fn cache_len(d: &Path) -> u64 {
    std::fs::metadata(d.join("Waveforms").join("Long.wav.peaks")).unwrap().len()
}

#[test]
fn paged_overviews_hold_little_and_draw_as_the_whole_ones() {
    let _t = turn();
    let d = dir("paged");
    let (path, file) = session(&d, 240);
    let e = open(&path);
    let a = source(&e);
    let want = whole(&file);
    assert!(a.disk_file().is_some());
    assert!(a.peaks.iter().all(Peaks::is_paged), "overviews paged");
    let held: usize = a.peaks.iter().map(Peaks::memory_bytes).sum();
    let all: usize = want.iter().map(Peaks::memory_bytes).sum();
    assert!(held * 200 < all, "{held} bytes held of {all}");
    let reads = peak_pages::reads();
    let mut coarse_first = 0;
    for (start, step, cols) in views(a.frames()) {
        for (ch, (p, w)) in a.peaks.iter().zip(&want).enumerate() {
            // As the drawing reads samples: a window around what is shown at fine zooms, none beyond.
            let from = (start as usize).saturating_sub(256);
            let window = if step <= 64.0 { a.window(from as i64, (step * cols as f64) as usize + 1_024).swap_remove(ch) } else { Vec::new() };
            let (got, at_once) = settled(p, &window, from, start, step, cols);
            coarse_first += usize::from(!at_once);
            let expect = (w.spans_in(&window, from, start, step, step, cols), w.rms_spans_in(&window, from, start, step, step, cols));
            assert!(got == expect, "channel {ch} at {start} step {step}");
        }
    }
    assert!(coarse_first > 0, "some views drew coarser first");
    let read = peak_pages::reads() - reads;
    assert!(read < 600, "{read} pages read for {} views", views(a.frames()).len() * 2);
    let (bytes, budget) = peak_pages::usage();
    assert!(bytes <= budget);
    // Exact queries wait for their pages.
    assert_eq!(a.peaks[1].minmax(&[], 12_345, 9_876_543), want[1].minmax(&[], 12_345, 9_876_543));
    assert!(a.peaks[0] == want[0]);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_old_cache_is_completed_and_old_readers_still_read_a_new_one() {
    let _t = turn();
    let d = dir("old");
    let (path, file) = session(&d, 60);
    drop(open(&path));
    let full = cache_len(&d);
    let frames = 60 * SR + 77;
    let n0 = frames.div_ceil(32) as u64;
    let level0_end = 32 + 2 * (16 + n0 * 12);
    assert!(full > level0_end, "every level is stored");
    // An old reader (level 0 only, whole) reads the new file.
    let buf = AudioBuffer::new(SR as u32, 2, frames);
    assert_eq!(soundcraft_engine::peak_cache::load(&d, &file, SR as u32, &buf), Some(whole(&file)));
    // As an older version writes it: level 0 alone.
    let cache = d.join("Waveforms").join("Long.wav.peaks");
    let f = std::fs::OpenOptions::new().write(true).open(&cache).unwrap();
    f.set_len(level0_end).unwrap();
    drop(f);
    let e = open(&path);
    let a = source(&e);
    assert!(a.peaks.iter().all(Peaks::is_paged), "an old cache pages too");
    assert_eq!(cache_len(&d), full, "completed in place");
    let want = whole(&file);
    let (got, _) = settled(&a.peaks[0], &[], 0, 100_000.5, 900.0, 1_500);
    assert!(got.0 == want[0].spans_in(&[], 0, 100_000.5, 900.0, 900.0, 1_500));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_cache_deleted_or_damaged_while_open_is_made_again() {
    let _t = turn();
    let d = dir("damaged");
    let (path, file) = session(&d, 90);
    let e = open(&path);
    let a = source(&e);
    let want = whole(&file);
    let cache = d.join("Waveforms").join("Long.wav.peaks");
    std::fs::remove_file(&cache).unwrap();
    let (got, _) = settled(&a.peaks[0], &[], 0, 1_000_000.0, 700.0, 1_800);
    assert!(got.0 == want[0].spans_in(&[], 0, 1_000_000.0, 700.0, 700.0, 1_800), "drawn right after the cache went");
    assert!(cache.is_file(), "made again");
    // Damaged (another length): made again or held in memory; drawn right either way, no crash.
    std::fs::write(&cache, b"FDWPEAK1 not a cache").unwrap();
    let (got, _) = settled(&a.peaks[1], &[], 0, 3_000_000.0, 2_000.0, 1_800);
    assert!(got.0 == want[1].spans_in(&[], 0, 3_000_000.0, 2_000.0, 2_000.0, 1_800));
    std::fs::write(&cache, vec![0u8; 100]).unwrap();
    let (got, _) = settled(&a.peaks[1], &[], 0, 200_000.0, 300.0, 1_800);
    assert!(got.0 == want[1].spans_in(&[], 0, 200_000.0, 300.0, 300.0, 1_800));
    drop(e);
    // A damaged cache at open: the overview is made again from the audio.
    std::fs::write(&cache, vec![7u8; 4096]).unwrap();
    let e = open(&path);
    assert!(source(&e).peaks.iter().all(Peaks::is_paged));
    assert!(source(&e).peaks[0] == want[0]);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn without_a_project_overviews_stay_whole_and_a_save_pages_them() {
    let _t = turn();
    let d = dir("unsaved");
    let (_, file) = session(&d, 30);
    let mut e = Engine::default();
    let r = e.execute("file.import_audio", &json!({"path": file.to_string_lossy()})).unwrap();
    let src = SourceId(r["imported"][0]["source"].as_u64().unwrap());
    let a = Arc::clone(e.session().pool.get(src).unwrap());
    assert!(a.disk_file().is_some() && !a.peaks.iter().any(Peaks::is_paged), "no project: held whole");
    let song = d.join("Song2").join("Song2.fdawm");
    e.execute("session.save_as", &json!({"path": song.to_string_lossy()})).unwrap();
    let a = Arc::clone(e.session().pool.get(src).unwrap());
    assert!(a.peaks.iter().all(Peaks::is_paged), "saved: paged from the project's cache");
    assert!(a.peaks[0] == whole(&file)[0]);
    // Generated audio in a saved project (a sample edit of a long file) is paged as it is made.
    let clip = e.session().tracks[0].clips()[0].id.0;
    e.execute("clip.sample_edit", &json!({"clip": clip, "op": "reverse", "file_start": 1_000, "file_end": 900_000})).unwrap();
    let s = e.session();
    let edited = s.pool.get(s.tracks[0].clips()[0].source().unwrap()).unwrap();
    assert!(!Arc::ptr_eq(edited, &a) && edited.disk_file().is_some());
    assert!(edited.peaks.iter().all(Peaks::is_paged), "generated audio paged");
    let _ = std::fs::remove_dir_all(&d);
}
