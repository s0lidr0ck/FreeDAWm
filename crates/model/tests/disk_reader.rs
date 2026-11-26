//! The streaming reader against a whole-file decode: every way of reading a file played from disk
//! (blocking reads, blocking and realtime cursors, windows) gives the decoded samples bit for bit
//! at any position, across block edges, for every PCM depth and channel count; a loop's start is
//! ready before the playhead wraps to it; and a block the audio thread finds missing is silence
//! for exactly that block, counted, never a wait.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat, decode, encode};
use soundcraft_model::disk::{self, BLOCK, DiskFile};
use soundcraft_model::{ChannelFormat, Clip, ClipId, Session, SourceAudio, SourceId, TrackKind};
use soundcraft_time::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("freedawm-disk-reader-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Noise-like material: every sample differs, so silence or a shifted read never passes.
fn signal(channels: usize, frames: usize, seed: u32) -> AudioBuffer {
    let mut x = seed.wrapping_mul(2_654_435_761).max(1);
    AudioBuffer {
        sample_rate: 48_000,
        channels: (0..channels)
            .map(|_| {
                (0..frames)
                    .map(|_| {
                        x ^= x << 13;
                        x ^= x >> 17;
                        x ^= x << 5;
                        (x as f32 / u32::MAX as f32) * 1.6 - 0.8 + 1e-3
                    })
                    .collect()
            })
            .collect(),
    }
}

fn write(d: &Path, name: &str, buf: &AudioBuffer, depth: BitDepth) -> (PathBuf, AudioBuffer) {
    let bytes = encode(buf, &EncodeOptions { format: FileFormat::Wav, bit_depth: depth, dither: false, bwf: None, bitrate_kbps: 0 }).unwrap();
    let p = d.join(name);
    std::fs::write(&p, &bytes).unwrap();
    (p, decode(&bytes, Some("wav")).unwrap().1)
}

fn bits(x: &[f32]) -> Vec<u32> {
    x.iter().map(|v| v.to_bits()).collect()
}

/// What a whole-file decode holds at `[start, start + n)` of `ch` (silence outside).
fn expected(want: &AudioBuffer, ch: usize, start: i64, n: usize) -> Vec<f32> {
    (0..n as i64).map(|i| usize::try_from(start + i).ok().and_then(|x| want.channels[ch].get(x).copied()).unwrap_or(0.0)).collect()
}

#[test]
fn random_seeks_through_every_reader_match_the_whole_file_decode() {
    let d = dir("seeks");
    let mut rng = 12_345u64;
    let mut next = |m: u64| {
        rng = rng.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (rng >> 33) % m.max(1)
    };
    for (k, (channels, depth)) in [(1, BitDepth::Int16), (6, BitDepth::Int24), (3, BitDepth::Int32), (2, BitDepth::Float32)].into_iter().enumerate() {
        let frames = BLOCK * 2 + 4_321 + k * 17;
        let (p, want) = write(&d, &format!("s{k}.wav"), &signal(channels, frames, k as u32 + 1), depth);
        let f = DiskFile::open(&p).unwrap();
        assert_eq!((f.channels(), f.frames()), (channels, frames));
        let audio = SourceAudio::from_disk_scanned(Arc::clone(&f));
        for _ in 0..60 {
            let ch = next(channels as u64) as usize;
            let start = next(frames as u64 + 2_000) as i64 - 1_000;
            let n = 1 + next(BLOCK as u64 + 3_000) as usize;
            let want_here = expected(&want, ch, start, n);
            // A blocking cursor (bounce, freeze): any position, negative ones too.
            let mut out = vec![9.0f32; n];
            audio.cursor(ch, false).fill(start, &mut out);
            assert_eq!(bits(&out), bits(&want_here), "cursor ch {ch} at {start} for {n}");
            // A window (analysis, Process).
            assert_eq!(bits(&audio.window(start, n)[ch]), bits(&want_here), "window ch {ch} at {start}");
            // A blocking read, from a non-negative position.
            if start >= 0 {
                let mut out = vec![9.0f32; n];
                let inside = audio.read(ch, start as usize, &mut out);
                assert_eq!(inside, frames.saturating_sub(start as usize).min(n));
                assert_eq!(bits(&out), bits(&want_here), "read ch {ch} at {start}");
            }
        }
        // Once read, the audio thread's cursor reads the same, position by position too.
        assert!(f.fully_cached());
        for ch in 0..channels {
            let mut rt = audio.cursor(ch, true);
            let mut out = vec![0.0f32; frames];
            rt.fill(0, &mut out);
            assert_eq!(bits(&out), bits(&want.channels[ch]));
            let mut rt = audio.cursor(ch, true);
            assert!((0..frames).step_by(1_009).all(|x| rt.get(x).map(f32::to_bits) == Some(want.channels[ch][x].to_bits())));
            assert_eq!(rt.get(frames), None, "past the end");
        }
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_loop_start_is_ready_before_the_playhead_wraps_to_it() {
    let d = dir("loop");
    let frames = BLOCK * 10;
    let (p, want) = write(&d, "loop.wav", &signal(2, frames, 77), BitDepth::Int24);
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Loop"));
    let audio = Arc::new(SourceAudio::from_disk(DiskFile::open(&p).unwrap(), SourceAudio::new(want.clone()).peaks).unwrap());
    s.pool.insert(SourceId(700), Arc::clone(&audio));
    let clip = Clip::audio(ClipId(s.alloc()), "Loop", SourceId(700), 0, 0, frames as i64);
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(clip);
    // A loop from block 2 to block 8; the playhead a moment before its end.
    let (l0, l1) = ((BLOCK * 2 + 100) as i64, (BLOCK * 8) as i64);
    s.edit.loop_playback = true;
    s.edit.loop_range = Some(Range::new(l0, l1));
    let file = audio.disk_file().unwrap();
    assert!(!file.fully_cached());
    assert!(disk::prepare(&s, l1 - 2_000, 0.5, Duration::from_secs(10)));
    // Right after the wrap the audio thread reads the loop start's samples, not silence.
    for ch in 0..2 {
        let mut out = vec![0.0f32; 12_000];
        audio.cursor(ch, true).fill(l0, &mut out);
        assert_eq!(bits(&out), bits(&expected(&want, ch, l0, 12_000)), "loop start, ch {ch}");
        let mut out = vec![0.0f32; 1_900];
        audio.cursor(ch, true).fill(l1 - 2_000, &mut out);
        assert_eq!(bits(&out), bits(&expected(&want, ch, l1 - 2_000, 1_900)), "before the wrap, ch {ch}");
    }
    assert!(!file.fully_cached(), "only what the loop needs first was read");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_block_the_audio_thread_misses_is_silence_for_that_block_alone_and_is_counted() {
    let d = dir("miss");
    let frames = BLOCK * 4;
    let (p, want) = write(&d, "miss.wav", &signal(1, frames, 5), BitDepth::Float32);
    let audio = SourceAudio::from_disk(DiskFile::open(&p).unwrap(), SourceAudio::new(want.clone()).peaks).unwrap();
    // Blocks 0 and 2 read (offline reads keep what they read); block 1 never asked for.
    let mut scratch = vec![0.0f32; 16];
    audio.read(0, 10, &mut scratch);
    audio.read(0, 2 * BLOCK + 10, &mut scratch);
    let before = disk::underruns();
    let (start, n) = (BLOCK - 100, BLOCK + 300);
    let mut out = vec![9.0f32; n];
    audio.cursor(0, true).fill(start as i64, &mut out);
    assert_eq!(bits(&out[..100]), bits(&want.channels[0][start..BLOCK]), "block 0 plays");
    assert!(out[100..100 + BLOCK].iter().all(|v| *v == 0.0), "block 1: silence, no wait");
    assert_eq!(bits(&out[100 + BLOCK..]), bits(&want.channels[0][2 * BLOCK..2 * BLOCK + 200]), "block 2 plays");
    assert!(disk::underruns() > before, "the miss is counted");
    assert!(disk::underruns_since_reset() <= disk::underruns());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn small_sources_stay_in_memory_by_the_engine_setting() {
    assert_eq!(disk::memory_threshold(), disk::DEFAULT_MEMORY_THRESHOLD);
    assert!(disk::keeps_in_memory(1_000, 2));
    assert!(!disk::keeps_in_memory(disk::DEFAULT_MEMORY_THRESHOLD, 1));
    assert!(!disk::keeps_in_memory(disk::DEFAULT_MEMORY_THRESHOLD / 2, 2), "frames times channels");
}
