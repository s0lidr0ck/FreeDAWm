//! Sessions played from disk: the same saved session opened with every source in memory and with
//! every source streamed bounces bit for bit the same. The in-memory threshold and the disk
//! cache are process-wide, so the tests in this file take turns.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat, encode};
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, Clip, ClipId, Fade, FadeShape, Session, Source, SourceId, TrackKind, disk};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static TURN: Mutex<()> = Mutex::new(());

fn turn() -> std::sync::MutexGuard<'static, ()> {
    TURN.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("freedawm-diskbudget-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("Audio Files")).unwrap();
    d
}

const SR: usize = 48_000;

/// Every sample differs (noise-like), so a gap or shifted read never passes.
fn noise(channels: usize, frames: usize, seed: u32) -> AudioBuffer {
    let mut x = seed.wrapping_mul(2_654_435_761).max(1);
    let channels = (0..channels)
        .map(|_| {
            (0..frames)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    (x as f32 / u32::MAX as f32) - 0.5
                })
                .collect()
        })
        .collect();
    AudioBuffer { sample_rate: SR as u32, channels }
}

/// Writes `buf` as `Audio Files/<name>` of project `d` and lists it as a saved source.
fn add_file(s: &mut Session, d: &Path, name: &str, buf: &AudioBuffer, depth: BitDepth) -> SourceId {
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: depth, dither: false, bwf: None, bitrate_kbps: 0 };
    std::fs::write(d.join("Audio Files").join(name), encode(buf, &opts).unwrap()).unwrap();
    let id = SourceId(s.alloc());
    s.sources.push(Source {
        id,
        name: name.trim_end_matches(".wav").to_string(),
        path: format!("Audio Files/{name}"),
        channels: buf.num_channels() as u16,
        frames: buf.frames() as u64,
        sample_rate: SR as u32,
        format: FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
    });
    id
}

fn clip(s: &mut Session, src: SourceId, offset: i64, start: i64, len: i64) -> Clip {
    Clip::audio(ClipId(s.alloc()), "c", src, offset, start, len)
}

/// A saved session in `d`: stereo 24-bit, mono 16-bit and float files (one below the in-memory
/// threshold), clips trimmed, moved, faded, gained, stretched, reversed and looped.
fn write_session(d: &Path) -> (PathBuf, i64) {
    let len = 6 * SR + 321;
    let mut s = Session::default();
    let a = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("A"));
    let b = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("B"));
    let c = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("C"));
    let sa = add_file(&mut s, d, "a.wav", &noise(2, len, 1), BitDepth::Int24);
    let sb = add_file(&mut s, d, "b.wav", &noise(1, len, 2), BitDepth::Int16);
    let sc = add_file(&mut s, d, "c.wav", &noise(1, len, 3), BitDepth::Float32);
    let tiny = add_file(&mut s, d, "tiny.wav", &noise(1, 9_000, 4), BitDepth::Int24);
    let mut ca = clip(&mut s, sa, 12_345, 1_000, (len - 40_000) as i64);
    ca.gain_db = -4.5;
    ca.fade_in = Fade { len: 4_800, shape: FadeShape::default(), curvature: 0.3 };
    ca.fade_out = Fade { len: 9_600, shape: FadeShape::default(), curvature: 0.0 };
    let mut cb = clip(&mut s, sb, 0, 20_000, (2 * SR) as i64);
    cb.stretch = 1.29;
    let mut cb2 = clip(&mut s, sb, (3 * SR) as i64, (3 * SR) as i64, SR as i64);
    cb2.reversed = true;
    let mut cc = clip(&mut s, sc, 70_000, 0, (5 * SR) as i64);
    cc.loop_len = 40_000;
    let ct = clip(&mut s, tiny, 0, (4 * SR) as i64, 9_000);
    s.track_mut(a).unwrap().playlist_mut().unwrap().clips.push(ca);
    let pb = s.track_mut(b).unwrap().playlist_mut().unwrap();
    pb.clips.push(cb);
    pb.clips.push(cb2);
    let pc = s.track_mut(c).unwrap().playlist_mut().unwrap();
    pc.clips.push(cc);
    pc.clips.push(ct);
    let path = d.join("Song.fdawm");
    std::fs::write(&path, s.to_json().unwrap()).unwrap();
    (path, len as i64)
}

/// Opens `path` with the in-memory threshold at `threshold`, bounces `[0, len)` to a float WAV
/// and returns its samples' bits, and how many sources stream.
fn open_and_bounce(path: &Path, threshold: usize, len: i64, out: &Path) -> (Vec<Vec<u32>>, usize, usize) {
    disk::set_memory_threshold(threshold);
    let mut e = Engine::default();
    e.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
    let s = e.session();
    let streamed = s.pool.ids().filter(|id| s.pool.get(*id).is_some_and(|a| a.disk_file().is_some())).count();
    let held = s.pool.ids().filter(|id| s.pool.get(*id).is_some_and(|a| a.in_memory().is_some())).count();
    e.execute("file.bounce_mix", &json!({"path": out.to_string_lossy(), "start": 0, "end": len, "bit_depth": 32})).unwrap();
    let (_, buf) = soundcraft_audio_io::decode(&std::fs::read(out).unwrap(), Some("wav")).unwrap();
    disk::set_memory_threshold(disk::DEFAULT_MEMORY_THRESHOLD);
    (buf.channels.iter().map(|c| c.iter().map(|v| v.to_bits()).collect()).collect(), streamed, held)
}

#[test]
fn a_session_bounces_bit_for_bit_the_same_from_memory_and_from_disk() {
    let _t = turn();
    let d = dir("both");
    let (path, len) = write_session(&d);
    let (mem, streamed, held) = open_and_bounce(&path, usize::MAX, len, &d.join("mem.wav"));
    assert_eq!((streamed, held), (0, 4), "everything in memory");
    let (all_disk, streamed, held) = open_and_bounce(&path, 0, len, &d.join("disk.wav"));
    assert_eq!((streamed, held), (4, 0), "everything from disk");
    let (default, streamed, held) = open_and_bounce(&path, disk::DEFAULT_MEMORY_THRESHOLD, len, &d.join("default.wav"));
    assert_eq!((streamed, held), (3, 1), "by default the tiny file stays in memory");
    assert!(mem[0].iter().any(|v| *v != 0), "the bounce holds audio");
    assert!(mem == all_disk, "streamed and in-memory renders differ");
    assert!(mem == default);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_wav_at_the_session_rate_is_imported_to_play_from_disk_and_saved_as_before() {
    let _t = turn();
    let d = dir("import");
    let frames = 10 * SR + 77;
    let buf = noise(2, frames, 21);
    let bwf = soundcraft_audio_io::BwfInfo { time_reference: 123_456_789, ..Default::default() };
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int24, dither: false, bwf: Some(bwf), bitrate_kbps: 0 };
    let file = d.join("Long Take.wav");
    std::fs::write(&file, encode(&buf, &opts).unwrap()).unwrap();
    let import = |threshold: usize| {
        disk::set_memory_threshold(threshold);
        let mut e = Engine::default();
        let r = e.execute("file.import_audio", &json!({"path": file.to_string_lossy()})).unwrap();
        disk::set_memory_threshold(disk::DEFAULT_MEMORY_THRESHOLD);
        let src = SourceId(r["imported"][0]["source"].as_u64().unwrap());
        assert_eq!(r["imported"][0]["frames"], frames);
        (e, src)
    };
    let (mut streamed, src) = import(disk::DEFAULT_MEMORY_THRESHOLD);
    let a = streamed.session().pool.get(src).unwrap();
    assert!(a.disk_file().is_some() && a.in_memory().is_none(), "plays from the file where it is");
    let info = streamed.session().source(src).unwrap();
    assert_eq!((info.time_reference, info.channels, info.frames), (123_456_789, 2, frames as u64));
    let (memory, msrc) = import(usize::MAX);
    assert!(memory.session().pool.get(msrc).unwrap().in_memory().is_some());
    let r = soundcraft_time::Range::new(0, frames as i64);
    let bits = |x: Vec<Vec<f32>>| x.into_iter().map(|c| c.into_iter().map(f32::to_bits).collect::<Vec<_>>()).collect::<Vec<_>>();
    assert!(bits(soundcraft_mix::render_range(streamed.session(), r, 512)) == bits(soundcraft_mix::render_range(memory.session(), r, 512)));
    // Saved (imports copied into the project): the copy holds the same audio, as float.
    let song = d.join("Song").join("Song.fdawm");
    streamed.execute("session.save_as", &json!({"path": song.to_string_lossy()})).unwrap();
    let saved = streamed.session().source(src).unwrap().path.clone();
    let (_, back) = soundcraft_audio_io::decode(&std::fs::read(d.join("Song").join(&saved)).unwrap(), Some("wav")).unwrap();
    let decoded = soundcraft_audio_io::decode(&std::fs::read(&file).unwrap(), Some("wav")).unwrap().1;
    assert!(bits(back.channels) == bits(decoded.channels), "{saved}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn sources_are_written_out_block_by_block_byte_for_byte_as_a_whole_encode() {
    let _t = turn();
    let d = dir("write");
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None, bitrate_kbps: 0 };
    for (k, (channels, frames)) in [(1usize, 70_001usize), (2, 200_000), (6, 66_000)].into_iter().enumerate() {
        let mut buf = noise(channels, frames, 10 + k as u32);
        // Out of range and not a number: clamped and zeroed as the whole encode does.
        buf.channels[0][5] = 3.0;
        buf.channels[0][6] = -7.5;
        buf.channels[0][7] = f32::NAN;
        let want = encode(&buf, &opts).unwrap();
        let mem = soundcraft_model::SourceAudio::new(buf.clone());
        let mut got = Vec::new();
        assert_eq!(soundcraft_engine::io::write_source_wav(&mem, &mut got).unwrap(), want.len() as u64);
        assert!(got == want, "{channels} ch from memory");
        // From disk, a 24-bit file: the same as encoding its decode.
        let p = d.join(format!("w{k}.wav"));
        std::fs::write(&p, encode(&buf, &EncodeOptions { bit_depth: BitDepth::Int24, ..opts.clone() }).unwrap()).unwrap();
        let decoded = soundcraft_audio_io::decode(&std::fs::read(&p).unwrap(), Some("wav")).unwrap().1;
        let streamed = soundcraft_model::SourceAudio::from_disk_scanned(disk::DiskFile::open(&p).unwrap());
        let mut got = Vec::new();
        soundcraft_engine::io::write_source_wav(&streamed, &mut got).unwrap();
        assert!(got == encode(&decoded, &opts).unwrap(), "{channels} ch from disk");
    }
    let _ = std::fs::remove_dir_all(&d);
}
