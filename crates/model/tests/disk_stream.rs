//! Audio played from disk: reads through the block cache are the file's samples exactly; the
//! audio thread's reads never block (a missing block is silence, counted and fetched); a cache
//! smaller than the session keeps working by evicting what was played longest ago.

use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat, decode, encode};
use soundcraft_model::disk::{self, BLOCK, DiskFile};
use soundcraft_model::{Clip, Session, SourceAudio, SourceId, TrackKind};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("freedawm-disk-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn signal(channels: usize, frames: usize, seed: u32) -> AudioBuffer {
    AudioBuffer {
        sample_rate: 48_000,
        channels: (0..channels).map(|c| (0..frames).map(|i| (i as f32 * 0.0031 + c as f32 + seed as f32).sin() * 0.8).collect()).collect(),
    }
}

fn write(d: &std::path::Path, name: &str, buf: &AudioBuffer, depth: BitDepth) -> (PathBuf, AudioBuffer) {
    let bytes = encode(buf, &EncodeOptions { format: FileFormat::Wav, bit_depth: depth, dither: false, bwf: None, bitrate_kbps: 0 }).unwrap();
    let p = d.join(name);
    std::fs::write(&p, &bytes).unwrap();
    (p, decode(&bytes, Some("wav")).unwrap().1)
}

fn same(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
}

#[test]
fn blocking_reads_and_cursors_read_the_decoded_samples_exactly() {
    let d = dir("exact");
    let frames = BLOCK * 3 + 1234;
    for (i, depth) in [BitDepth::Int16, BitDepth::Int24, BitDepth::Float32].into_iter().enumerate() {
        let (p, want) = write(&d, &format!("x{i}.wav"), &signal(2, frames, i as u32), depth);
        let f = DiskFile::open(&p).unwrap();
        assert_eq!((f.channels(), f.frames()), (2, frames));
        // Straddling block edges, and past the end (silence).
        let mut out = vec![1.0f32; 5000];
        for start in [0usize, BLOCK - 7, 2 * BLOCK - 2500, frames - 100] {
            let n = f.read(1, start, &mut out);
            let inside = (frames - start).min(out.len());
            assert_eq!(n, inside);
            assert!(same(&out[..inside], &want.channels[1][start..start + inside]));
            assert!(out[inside..].iter().all(|x| *x == 0.0));
        }
        let audio = SourceAudio::from_disk_scanned(Arc::clone(&f));
        let mem = SourceAudio::new(want.clone());
        assert_eq!(audio.peaks, mem.peaks, "waveforms made from the file in blocks");
        assert!(same(&audio.buffer().channels[0], &want.channels[0]));
        let mut c = audio.cursor(0, false);
        assert!((0..frames).step_by(997).all(|x| c.get(x).map(f32::to_bits) == Some(want.channels[0][x].to_bits())));
        assert_eq!(c.get(frames), None);
        assert_eq!(audio.window(-10, 20)[0][..10], [0.0; 10]);
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_audio_threads_cursor_never_blocks_and_a_miss_is_fetched() {
    let d = dir("realtime");
    let frames = BLOCK * 4;
    let (p, want) = write(&d, "rt.wav", &signal(1, frames, 7), BitDepth::Float32);
    let audio = SourceAudio::from_disk(DiskFile::open(&p).unwrap(), vec![soundcraft_audio_io::peaks::Peaks::build(&want.channels[0])]).unwrap();
    let at = BLOCK * 2 + 5;
    let before = disk::underruns();
    let mut c = audio.cursor(0, true);
    assert_eq!(c.get(at), Some(0.0), "not loaded yet: silence");
    assert!(disk::underruns() > before, "counted");
    // The disk thread fetches what was asked for.
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let mut c = audio.cursor(0, true);
        if c.get(at).map(f32::to_bits) == Some(want.channels[0][at].to_bits()) {
            break;
        }
        assert!(Instant::now() < until, "the missed block arrives");
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn prepare_loads_what_playing_from_the_playhead_needs() {
    let d = dir("prepare");
    let frames = BLOCK * 8;
    let (p, want) = write(&d, "song.wav", &signal(2, frames, 3), BitDepth::Int24);
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, soundcraft_model::ChannelFormat::Stereo, Some("Vox"));
    let audio = Arc::new(SourceAudio::from_disk_scanned(DiskFile::open(&p).unwrap()));
    s.pool.insert(SourceId(500), Arc::clone(&audio));
    let clip = Clip::audio(soundcraft_model::ClipId(s.alloc()), "Vox", SourceId(500), 0, 1000, frames as i64);
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(clip);
    let pos = 1000 + (BLOCK * 5) as i64;
    assert!(disk::prepare(&s, pos, 1.0, Duration::from_secs(10)));
    // Every sample of the next second reads without waiting.
    let from = BLOCK * 5;
    for ch in 0..2 {
        let mut c = audio.cursor(ch, true);
        assert!((from..from + 48_000).all(|x| c.get(x).map(f32::to_bits) == Some(want.channels[ch][x].to_bits())));
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_cache_takes_a_quarter_of_memory_within_bounds() {
    const GIB: u64 = 1 << 30;
    assert_eq!(disk::budget_for(Some(32 * GIB)) as u64, disk::MAX_BUDGET, "capped");
    assert_eq!(disk::budget_for(Some(8 * GIB)) as u64, 2 * GIB, "a quarter");
    assert_eq!(disk::budget_for(Some(GIB)) as u64, 512 << 20, "floored");
    assert_eq!(disk::budget_for(None) as u64, 2 * GIB, "unknown");
}
