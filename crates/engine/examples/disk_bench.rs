//! Disk streaming measurements on a synthetic session of mono 32-bit float WAVs:
//!
//! `cargo run --release -p soundcraft-engine --example disk_bench -- make <dir> <tracks> <minutes>`
//! writes the project (`<dir>/Bench/Bench.fdawm` and its `Audio Files/`);
//! `… -- open <dir>` opens it and reports the time (memory is read from outside, e.g. the peak
//! working set), then plays 20 s from the middle in real time on a thread marked as the audio
//! thread and reports underruns; `… -- render <dir> <seconds>` renders the first seconds offline
//! from disk (cached) and from memory, checks they are bit-identical and times both.
//! `FREEDAWM_STREAM=0` opens the old way (every file decoded into memory).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileEncoder, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, Clip, Session, Source, SourceAudio, SourceId, TrackKind, disk};
use soundcraft_time::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| ".".into()));
    match args.first().map(String::as_str) {
        Some("make") => make(&dir, args.get(2).and_then(|a| a.parse().ok()).unwrap_or(16), args.get(3).and_then(|a| a.parse().ok()).unwrap_or(60)),
        Some("open") => open(&dir),
        Some("render") => render(&dir, args.get(2).and_then(|a| a.parse().ok()).unwrap_or(120)),
        _ => eprintln!("usage: disk_bench make|open|render <dir> …"),
    }
}

fn session_path(dir: &Path) -> PathBuf {
    dir.join("Bench").join("Bench.fdawm")
}

fn make(dir: &Path, tracks: usize, minutes: usize) {
    let t0 = Instant::now();
    let audio = dir.join("Bench").join("Audio Files");
    std::fs::create_dir_all(&audio).unwrap();
    let frames = minutes * 60 * 48_000;
    let mut s = Session::new("Bench", soundcraft_time::SampleRate::HZ_48000);
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None, bitrate_kbps: 0 };
    for k in 0..tracks {
        let name = format!("Track {:02}", k + 1);
        let path = audio.join(format!("{name}.wav"));
        let f = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(&path).unwrap());
        let mut enc = FileEncoder::new(f, &opts, 1, 48_000, None).unwrap();
        let f0 = 0.002 + k as f32 * 0.0003;
        let mut at = 0usize;
        while at < frames {
            let n = (frames - at).min(1 << 16);
            let block = vec![(at..at + n).map(|i| (i as f32 * f0).sin() * 0.25).collect::<Vec<f32>>()];
            enc.write(&block, n).unwrap();
            at += n;
        }
        enc.finish().unwrap();
        let id = SourceId(s.alloc());
        s.sources.push(Source {
            id,
            name: name.clone(),
            path: format!("Audio Files/{name}.wav"),
            channels: 1,
            frames: frames as u64,
            sample_rate: 48_000,
            format: FileFormat::Wav,
            time_reference: 0,
            unsaved: false,
        });
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(&name));
        let cid = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(cid, name, id, 0, 0, frames as i64));
    }
    std::fs::write(session_path(dir), s.to_json().unwrap()).unwrap();
    println!("made {tracks} tracks × {minutes} min in {:.1} s", t0.elapsed().as_secs_f64());
}

fn open(dir: &Path) {
    let t0 = Instant::now();
    let mut e = Engine::default();
    e.execute("session.open", &json!({"path": session_path(dir).to_string_lossy()})).unwrap();
    let opened = t0.elapsed().as_secs_f64();
    let s = e.session_arc();
    let streamed = s.pool.ids().filter(|id| s.pool.get(*id).is_some_and(|a| a.disk_file().is_some())).count();
    let frames = s.sources.first().map_or(0, |x| x.frames) as i64;
    println!("OPENED in {opened:.2} s: {} sources, {streamed} played from disk", s.sources.len());
    // Hold still a moment so the peak working set after opening can be read from outside.
    std::thread::sleep(Duration::from_secs(2));
    // Play 20 s from the middle in real time, as the audio thread does.
    let from = frames / 2;
    disk::focus(Some(Arc::clone(&s)));
    let t1 = Instant::now();
    let ready = disk::prepare(&s, from, 1.0, Duration::from_millis(300));
    println!("prepare 1 s at the playhead: {:.0} ms (complete: {ready})", t1.elapsed().as_secs_f64() * 1000.0);
    let before = disk::underruns();
    let player = std::thread::spawn({
        let s = Arc::clone(&s);
        move || {
            soundcraft_mix::mark_audio_thread();
            let mut eng = soundcraft_mix::MixEngine::new(48_000.0, 512);
            let mut out = vec![vec![0.0f32; 512]; 2];
            let mut pos = from;
            let start = Instant::now();
            let mut worst = Duration::ZERO;
            for k in 0..(20 * 48_000 / 512) {
                let b = Instant::now();
                disk::set_playhead(pos, true);
                eng.render(&s, pos, 512, &mut out);
                worst = worst.max(b.elapsed());
                pos += 512;
                let due = start + Duration::from_secs_f64((k + 1) as f64 * 512.0 / 48_000.0);
                if let Some(wait) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait);
                }
            }
            worst
        }
    });
    let worst = player.join().unwrap();
    let (held, limit) = disk::usage();
    println!(
        "played 20 s from {:.0} min: {} underruns, worst block {:.2} ms (budget 10.7 ms); cache {:.0} MiB of {:.0} MiB",
        from as f64 / 48_000.0 / 60.0,
        disk::underruns() - before,
        worst.as_secs_f64() * 1000.0,
        held as f64 / 1048576.0,
        limit as f64 / 1048576.0
    );
    std::thread::sleep(Duration::from_secs(2));
}

fn render(dir: &Path, secs: i64) {
    let mut e = Engine::default();
    e.execute("session.open", &json!({"path": session_path(dir).to_string_lossy()})).unwrap();
    let disk_s = e.session().clone();
    let r = Range::new(0, secs * 48_000);
    // Everything rendered is cached first (a song that fits plays from the cache).
    assert!(disk::prepare(&disk_s, 0, secs as f64 + 1.0, Duration::from_secs(120)));
    let mut mem_s = disk_s.clone();
    for id in disk_s.pool.ids().collect::<Vec<_>>() {
        let a = disk_s.pool.get(id).unwrap();
        let buf: AudioBuffer = a.buffer().into_owned();
        mem_s.pool.insert(id, Arc::new(SourceAudio::new(buf)));
    }
    let time = |s: &Session| {
        let mut best = f64::MAX;
        let mut out = Vec::new();
        for _ in 0..3 {
            let t = Instant::now();
            out = soundcraft_mix::render_range(s, r, 512);
            best = best.min(t.elapsed().as_secs_f64());
        }
        (best, out)
    };
    let (tm, from_mem) = time(&mem_s);
    let (td, from_disk) = time(&disk_s);
    let same =
        from_mem.len() == from_disk.len() && from_mem.iter().zip(&from_disk).all(|(a, b)| a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits()));
    println!(
        "render {secs} s of {} tracks: memory {:.3} s, disk (cached) {:.3} s ({:+.1}%), bit-identical: {same}",
        disk_s.tracks.len(),
        tm,
        td,
        (td / tm - 1.0) * 100.0
    );
}
