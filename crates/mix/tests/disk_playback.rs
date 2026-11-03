//! A session whose audio plays from disk renders bit-identically to the same session decoded into
//! memory: offline (blocking reads), and on the audio thread once the blocks are cached. On the
//! audio thread a block not cached yet is silence for that block, counted, and plays once the
//! disk thread has fetched it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat, decode, encode};
use soundcraft_model::disk::{self, DiskFile};
use soundcraft_model::{ChannelFormat, CleanAlt, Clip, ClipId, Session, SourceAudio, SourceId, TrackKind};
use soundcraft_time::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const SR: usize = 48_000;

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("freedawm-mixdisk-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn tone(channels: usize, frames: usize, f: f32) -> AudioBuffer {
    AudioBuffer {
        sample_rate: SR as u32,
        channels: (0..channels).map(|c| (0..frames).map(|i| (i as f32 * f * (1.0 + c as f32 * 0.1)).sin() * 0.5).collect()).collect(),
    }
}

/// The audio written as a WAV in `d`, played from disk and from memory.
fn both(d: &Path, name: &str, buf: &AudioBuffer, depth: BitDepth) -> (Arc<SourceAudio>, Arc<SourceAudio>) {
    let bytes = encode(buf, &EncodeOptions { format: FileFormat::Wav, bit_depth: depth, dither: false, bwf: None }).unwrap();
    let p = d.join(name);
    std::fs::write(&p, &bytes).unwrap();
    let mem = SourceAudio::new(decode(&bytes, Some("wav")).unwrap().1);
    let file = SourceAudio::from_disk(DiskFile::open(&p).unwrap(), mem.peaks.clone()).unwrap();
    (Arc::new(file), Arc::new(mem))
}

/// Three tracks: a stereo clip trimmed and moved, a stretched mono clip, and a clip blended with a
/// Clean Vocal alternate; the same session with every source from disk and from memory.
fn sessions(d: &Path) -> (Session, Session, i64) {
    let len = 5 * SR + 777;
    let mut disk_s = Session::default();
    let a = disk_s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("A"));
    let b = disk_s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("B"));
    let c = disk_s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("C"));
    let mut mem_s = disk_s.clone();
    let srcs = [
        both(d, "a.wav", &tone(2, len, 0.01), BitDepth::Float32),
        both(d, "b.wav", &tone(1, len, 0.003), BitDepth::Int24),
        both(d, "c.wav", &tone(1, len, 0.02), BitDepth::Int16),
        both(d, "c-clean.wav", &tone(1, len / 2, 0.005), BitDepth::Float32),
    ];
    for (i, (disk_a, mem_a)) in srcs.iter().enumerate() {
        disk_s.pool.insert(SourceId(100 + i as u64), Arc::clone(disk_a));
        mem_s.pool.insert(SourceId(100 + i as u64), Arc::clone(mem_a));
    }
    for s in [&mut disk_s, &mut mem_s] {
        let mut ca = Clip::audio(ClipId(1), "A", SourceId(100), 12_345, 1_000, (len - 20_000) as i64);
        ca.gain_db = -3.0;
        let mut cb = Clip::audio(ClipId(2), "B", SourceId(101), 0, 30_000, (2 * SR) as i64);
        cb.stretch = 1.37;
        let mut cc = Clip::audio(ClipId(3), "C", SourceId(102), 1_000, 0, (4 * SR) as i64);
        cc.clean = Some(CleanAlt { source: SourceId(103), original: SourceId(102), origin: 40_000, mix: 0.6, bypass: false });
        s.track_mut(a).unwrap().playlist_mut().unwrap().clips.push(ca);
        s.track_mut(b).unwrap().playlist_mut().unwrap().clips.push(cb);
        s.track_mut(c).unwrap().playlist_mut().unwrap().clips.push(cc);
    }
    (disk_s, mem_s, len as i64)
}

fn bits(x: &[Vec<f32>]) -> Vec<Vec<u32>> {
    x.iter().map(|c| c.iter().map(|v| v.to_bits()).collect()).collect()
}

#[test]
fn offline_renders_from_disk_are_bit_identical() {
    let d = dir("offline");
    let (disk_s, mem_s, len) = sessions(&d);
    let r = Range::new(0, len);
    let from_disk = soundcraft_mix::render_range(&disk_s, r, 512);
    let from_mem = soundcraft_mix::render_range(&mem_s, r, 512);
    assert!(from_mem.iter().flatten().any(|v| v.abs() > 0.1), "something plays");
    assert_eq!(bits(&from_disk), bits(&from_mem));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn realtime_renders_from_a_prepared_cache_are_bit_identical_and_a_miss_recovers() {
    let d = dir("realtime");
    let (disk_s, mem_s, len) = sessions(&d);
    let r = Range::new(0, len);
    let from_mem = soundcraft_mix::render_range(&mem_s, r, 512);
    // On the audio thread, before anything is cached: silence where blocks are missing, counted.
    let before = disk::underruns();
    let cold = std::thread::spawn({
        let s = disk_s.clone();
        move || {
            soundcraft_mix::mark_audio_thread();
            soundcraft_mix::render_range(&s, Range::new(0, 4096), 512)
        }
    })
    .join()
    .unwrap();
    assert!(disk::underruns() > before, "misses counted");
    assert!(cold.iter().flatten().all(|v| *v == 0.0), "a miss is silence, never a wait");
    // Everything loaded: identical.
    assert!(disk::prepare(&disk_s, 0, len as f64 / SR as f64 + 1.0, Duration::from_secs(30)));
    let warm = std::thread::spawn({
        let s = disk_s.clone();
        move || {
            soundcraft_mix::mark_audio_thread();
            soundcraft_mix::render_range(&s, r, 512)
        }
    })
    .join()
    .unwrap();
    assert_eq!(bits(&warm), bits(&from_mem));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_disk_thread_reads_ahead_of_the_playhead_of_the_focused_session() {
    let d = dir("ahead");
    let (disk_s, _, _) = sessions(&d);
    let s = Arc::new(disk_s);
    disk::focus(Some(Arc::clone(&s)));
    disk::set_playhead(2 * SR as i64, true);
    let until = Instant::now() + Duration::from_secs(20);
    let a = s.pool.get(SourceId(100)).unwrap();
    // Track A's clip at 2 s plays source position 2 s - 1000 + 12345.
    let at = 2 * SR - 1_000 + 12_345;
    while !a.disk_file().unwrap().fully_cached() {
        assert!(Instant::now() < until, "the session is read into the cache while there is room");
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut c = a.cursor(0, true);
    assert!(c.get(at).is_some());
    disk::focus(None);
    let _ = std::fs::remove_dir_all(&d);
}
