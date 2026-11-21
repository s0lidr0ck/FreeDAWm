//! Sessions played from disk: the same saved session opened with every source in memory and with
//! every source streamed bounces bit for bit the same. The in-memory threshold and the disk
//! cache are process-wide, so the tests in this file take turns.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use soundcraft_audio_io::peaks::Peaks;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat, encode};
use soundcraft_engine::{Engine, peak_pages};
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

/// Decoded audio bytes a session's pool holds in memory.
fn pool_bytes(s: &Session) -> usize {
    s.pool.ids().filter_map(|id| s.pool.get(id)).filter_map(|a| a.in_memory()).map(|b| b.frames() * b.num_channels() * 4).sum()
}

/// Overview bytes a session's pool holds itself (for a source played from disk in a project, the
/// coarse levels only; the fine ones are in the page cache, [`peak_pages::usage`]).
fn overview_bytes(s: &Session) -> usize {
    s.pool.ids().filter_map(|id| s.pool.get(id)).map(|a| a.peaks.iter().map(Peaks::memory_bytes).sum::<usize>()).sum()
}

/// What overviews held whole would take: 12 bytes per 32 samples, and a third more for the
/// coarser levels.
fn whole_overview_bytes(s: &Session) -> usize {
    s.pool.ids().filter_map(|id| s.pool.get(id)).map(|a| a.peaks.iter().map(|p| p.len_samples() / 32 * 12 * 4 / 3).sum::<usize>()).sum()
}

/// Draws every source's waveform as the Edit window would, `screens` screenfuls of 1800 columns
/// at each of a few zooms spread across the session (each asked again until its pages are in),
/// checking the overviews' memory stays within `cap` throughout; the most it held.
fn draw_through(s: &Session, frames: usize, screens: usize, cap: usize) -> usize {
    let mut most = 0;
    for step in [40.0, 700.0, 3_000.0, 30_000.0, 300_000.0] {
        let cols = 1_800usize;
        let span = step * cols as f64;
        for k in 0..screens {
            let start = (frames as f64 - span).max(0.0) * k as f64 / (screens - 1).max(1) as f64;
            for id in s.pool.ids() {
                let Some(a) = s.pool.get(id) else { continue };
                for p in &a.peaks {
                    let mut tries = 0;
                    while !p.spans_in_checked(&[], 0, start, step, step, cols).1 {
                        tries += 1;
                        assert!(tries < 50, "pages never came");
                        peak_pages::wait_idle(std::time::Duration::from_secs(10));
                    }
                }
            }
            let held = overview_bytes(s) + peak_pages::usage().0;
            assert!(held <= cap, "drawing at step {step}: overviews hold {held} bytes, cap {cap}");
            most = most.max(held);
        }
    }
    most
}

/// A saved session of `tracks` mono 16-bit files of `secs` seconds each in `d`, every clip whole
/// from 0 at its own gain.
fn long_session(d: &Path, tracks: usize, secs: usize) -> (PathBuf, i64) {
    let frames = secs * SR;
    let mut s = Session::default();
    for k in 0..tracks {
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(&format!("T{k}")));
        let src = add_file(&mut s, d, &format!("t{k}.wav"), &noise(1, frames, 100 + k as u32), BitDepth::Int16);
        let mut c = clip(&mut s, src, 0, 0, frames as i64);
        c.gain_db = -12.0 - k as f32 * 0.5;
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(c);
    }
    let path = d.join("Long.fdawm");
    std::fs::write(&path, s.to_json().unwrap()).unwrap();
    (path, frames as i64)
}

/// The process's peak resident memory, where the system says (Linux).
fn peak_rss() -> String {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|t| t.lines().find(|l| l.starts_with("VmHWM")).map(|l| l.trim_start_matches("VmHWM:").trim().to_string()))
        .unwrap_or_else(|| "unknown".into())
}

/// The decoded audio held (the cache's and the pool's) is within `cap`; `peak` keeps the most.
fn check(s: &Session, cap: usize, peak: &mut usize, when: &str) {
    let (held, limit) = disk::usage();
    assert!(limit <= cap && held + pool_bytes(s) <= cap, "{when}: {held} bytes of audio held, cap {cap}");
    *peak = (*peak).max(held);
}

/// Opens a session whose audio is many times the cache's cap, plays `play_secs` of it through the
/// disk thread as the audio thread would (at twice real time) and bounces all of it, checking the
/// decoded audio held never goes over the cap; with `compare`, also opens it all in memory (the
/// "before") and checks the bounce is the same bit for bit.
/// The overviews are checked too: only their coarse levels are held, and drawing the session
/// through at several zooms keeps them and the page cache (its budget `pages`) within
/// `overview_cap`.
fn within_budget(tag: &str, tracks: usize, secs: usize, cap: usize, play_secs: usize, compare: bool, (pages, overview_cap): (usize, usize)) {
    let _t = turn();
    peak_pages::set_budget_bytes(pages);
    let d = dir(tag);
    let (path, len) = long_session(&d, tracks, secs);
    // The peak so far is the test's own (each file made whole, then encoded); from here on it is
    // the engine's (Linux lets the high-water mark be reset).
    let setup_peak = peak_rss();
    let _ = std::fs::write("/proc/self/clear_refs", "5");
    let whole = tracks * secs * SR * 4;
    assert!(whole >= cap * 8, "the session is many times the cap");
    let bits = |p: &Path| -> Vec<Vec<u32>> {
        let (_, b) = soundcraft_audio_io::decode(&std::fs::read(p).unwrap(), Some("wav")).unwrap();
        b.channels.iter().map(|c| c.iter().map(|v| v.to_bits()).collect()).collect()
    };
    // Before: every source decoded into memory.
    let reference = compare.then(|| {
        disk::set_memory_threshold(usize::MAX);
        let mut e = Engine::default();
        e.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
        disk::set_memory_threshold(disk::DEFAULT_MEMORY_THRESHOLD);
        let held = pool_bytes(e.session());
        assert_eq!(held, whole);
        let out = d.join("mem.wav");
        e.execute("file.bounce_mix", &json!({"path": out.to_string_lossy(), "start": 0, "end": len, "bit_depth": 32})).unwrap();
        println!("{tag}: in memory, the pool holds {} MB of decoded audio", held / 1_000_000);
        let b = bits(&out);
        let _ = std::fs::remove_file(&out);
        b
    });
    // After: streamed through a cache capped far below that.
    disk::set_limit_bytes(cap);
    let mut e = Engine::default();
    e.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
    let s = e.session_arc();
    assert!(s.pool.ids().all(|id| s.pool.get(id).is_some_and(|a| a.disk_file().is_some())), "every source streams");
    assert!(s.pool.ids().all(|id| s.pool.get(id).is_some_and(|a| a.peaks.iter().all(Peaks::is_paged))), "every overview is paged");
    assert_eq!(pool_bytes(&s), 0);
    let whole_overviews = whole_overview_bytes(&s);
    assert!(overview_bytes(&s) + peak_pages::usage().0 <= overview_cap && overview_cap * 4 <= whole_overviews, "overviews held at open");
    let mut peak = 0usize;
    // Play: the audio thread's reads, 4800 frames at a time, at twice real time.
    disk::focus(Some(std::sync::Arc::clone(&s)));
    assert!(disk::prepare(&s, 0, 1.0, std::time::Duration::from_secs(20)));
    let before = disk::underruns();
    let mut silent = 0usize;
    let step = 4_800usize;
    let audio: Vec<_> = s.pool.ids().filter_map(|id| s.pool.get(id).cloned()).collect();
    let mut out = vec![0.0f32; step];
    for pos in (0..play_secs * SR).step_by(step) {
        disk::set_playhead(pos as i64, true);
        for a in &audio {
            a.cursor(0, true).fill(pos as i64, &mut out);
            silent += out.iter().filter(|v| **v == 0.0).count();
        }
        check(e.session(), cap, &mut peak, "playing");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let late = disk::underruns() - before;
    disk::set_playhead(0, false);
    disk::focus(None);
    assert_eq!(late, 0, "no block came late");
    assert!(silent < tracks * play_secs * 4, "the audio played, not silence ({silent} zero samples)");
    // Bounce all of it, offline.
    let out = d.join("disk.wav");
    e.execute("file.bounce_mix", &json!({"path": out.to_string_lossy(), "start": 0, "end": len, "bit_depth": 32})).unwrap();
    check(e.session(), cap, &mut peak, "bounced");
    // Draw it through: scrolling and zooming over all of it reads only what is shown.
    let drawn = draw_through(&s, len as usize, 4, overview_cap);
    println!(
        "{tag}: {} MB of audio played and bounced from disk; cache cap {} MB, peak held {} MB; overviews {} kB held ({} MB whole), at most {} kB with pages while drawn; process peak {} (writing the test's files: {})",
        whole / 1_000_000,
        cap / 1_000_000,
        peak / 1_000_000,
        overview_bytes(&s) / 1_000,
        whole_overviews / 1_000_000,
        drawn / 1_000,
        peak_rss(),
        setup_peak,
    );
    if let Some(reference) = reference {
        assert!(bits(&out) == reference, "the streamed bounce differs from the in-memory one");
    }
    drop(e);
    disk::set_limit_bytes(disk::budget_bytes());
    peak_pages::set_budget_bytes(peak_pages::DEFAULT_BUDGET);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_session_eight_times_the_cache_plays_and_bounces_within_it() {
    // 6 tracks of 4 minutes: 276 MB decoded, against a 32 MiB cache; overviews (35 MB whole)
    // within 6 MiB with a 4 MiB page cache.
    within_budget("budget", 6, 240, 32 << 20, 8, true, (4 << 20, 6 << 20));
}

/// The same at session size: 16 tracks of 30 minutes (5.5 GB decoded, 2.8 GB of files written to
/// the temp folder) against a 64 MiB cache; overviews (691 MB whole) within 24 MiB with a 16 MiB
/// page cache. Slow: `cargo test -p soundcraft-engine --test disk_budget -- --ignored`.
#[test]
#[ignore]
fn a_long_multitrack_session_plays_and_bounces_within_a_small_cache() {
    within_budget("budget-big", 16, 1_800, 64 << 20, 8, false, (16 << 20, 24 << 20));
}

/// The clip's file as it is now (every channel, whole).
fn clip_audio(e: &Engine) -> (Vec<Vec<u32>>, bool) {
    let s = e.session();
    let c = &s.tracks[0].clips()[0];
    let a = s.pool.get(c.source().unwrap()).unwrap();
    let w = a.window(0, a.frames());
    (w.iter().map(|c| c.iter().map(|v| v.to_bits()).collect()).collect(), a.disk_file().is_some())
}

#[test]
fn sample_edits_of_a_long_file_are_written_to_disk_and_match_the_in_memory_edit() {
    let _t = turn();
    let d = dir("edits");
    let frames = 5 * SR;
    let file = d.join("Long.wav");
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int24, dither: false, bwf: None, bitrate_kbps: 0 };
    std::fs::write(&file, encode(&noise(2, frames, 31), &opts).unwrap()).unwrap();
    let edits = [
        json!({"cmd": "clip.draw_samples", "at": 100_000, "values": (0..2_000).map(|i| (i as f64 * 0.01).sin() * 0.9).collect::<Vec<_>>(), "channel": 1}),
        json!({"cmd": "clip.sample_edit", "op": "reverse", "file_start": 30_000, "file_end": 70_001}),
        json!({"cmd": "clip.sample_edit", "op": "gain", "file_start": 1_000, "file_end": 9_000, "db": -6.0, "channel": 0}),
        json!({"cmd": "clip.sample_edit", "op": "cut", "file_start": 150_000, "file_end": 160_000}),
        json!({"cmd": "clip.sample_edit", "op": "paste", "file_start": 10, "file_end": 10}),
        json!({"cmd": "clip.sample_edit", "op": "silence", "file_start": 200_000, "file_end": 200_500}),
        json!({"cmd": "clip.sample_edit", "op": "invert", "file_start": 0, "file_end": frames}),
    ];
    let run = |saved: bool| {
        let mut e = Engine::default();
        e.execute("file.import_audio", &json!({"path": file.to_string_lossy()})).unwrap();
        if saved {
            let song = d.join("Song").join("Song.fdawm");
            e.execute("session.save_as", &json!({"path": song.to_string_lossy()})).unwrap();
        }
        let mut after = Vec::new();
        for edit in &edits {
            let clip = e.session().tracks[0].clips()[0].id.0;
            let mut p = edit.clone();
            p["clip"] = json!(clip);
            let cmd = p["cmd"].as_str().unwrap().to_string();
            e.execute(&cmd, &p).unwrap();
            after.push(clip_audio(&e));
        }
        after
    };
    let on_disk = run(true);
    let in_memory = run(false);
    for (i, ((disk_bits, streamed), (mem_bits, _))) in on_disk.iter().zip(&in_memory).enumerate() {
        assert!(*streamed, "edit {i}: the new file plays from disk");
        assert!(disk_bits == mem_bits, "edit {i}: the file written block by block differs from the in-memory edit");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_rate_change_writes_each_converted_source_to_disk_as_it_goes() {
    let _t = turn();
    let d = dir("rate");
    let mut e = Engine::default();
    for k in 0..3 {
        let file = d.join(format!("s{k}.wav"));
        let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None, bitrate_kbps: 0 };
        std::fs::write(&file, encode(&noise(1, 3 * SR, 40 + k), &opts).unwrap()).unwrap();
        e.execute("file.import_audio", &json!({"path": file.to_string_lossy()})).unwrap();
    }
    e.execute("session.save_as", &json!({"path": d.join("Song").join("Song.fdawm").to_string_lossy()})).unwrap();
    let r = e.execute("session.set_sample_rate", &json!({"rate": 96_000})).unwrap();
    assert_eq!(r["sources"], 3);
    let s = e.session();
    for t in &s.tracks {
        let a = s.pool.get(t.clips()[0].source().unwrap()).unwrap();
        assert!(a.disk_file().is_some() && a.in_memory().is_none(), "converted and played from disk");
        assert_eq!((a.sample_rate(), a.frames()), (96_000, 6 * SR));
    }
    let _ = std::fs::remove_dir_all(&d);
}
