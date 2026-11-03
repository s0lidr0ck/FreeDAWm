//! Transcribe Lyrics speed and output on a file, for tuning:
//! `cargo run --release -p soundcraft-ml --example lyrics_bench -- IN.wav [WEIGHTS.bin] [--keep-silence]`
//! (`SOUNDCRAFT_WHISPER_THREADS` sets the threads). WEIGHTS is a file in the model folder (default:
//! the card's f16 model).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use soundcraft_ml::lyrics::{self, LyricsOptions, LyricsQuality};
use soundcraft_ml::{Ctx, models};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let wav = args.first().expect("IN.wav");
    let (_, buf) = soundcraft_audio_io::decode(&std::fs::read(wav).unwrap(), Some("wav")).unwrap();
    let mut m = models::load(&models::models_dir(), lyrics::ID).unwrap();
    if let Some(f) = args.get(1).filter(|a| !a.starts_with("--")) {
        m.card.file = f.clone();
    }
    let opts = LyricsOptions { quality: LyricsQuality::Best, skip_silence: !args.iter().any(|a| a == "--keep-silence"), ..LyricsOptions::default() };
    let len = buf.channels[0].len() as f64 / f64::from(buf.sample_rate);
    let t0 = std::time::Instant::now();
    // `--cancel-after S`: cancel S seconds after transcription starts, and time the stop.
    let cancel_after: Option<f64> = args.iter().position(|a| a == "--cancel-after").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok());
    let cancel = soundcraft_ml::Cancel::new();
    let (c2, started) = (cancel.clone(), std::sync::Arc::new(std::sync::Mutex::new(None::<std::time::Instant>)));
    let s2 = std::sync::Arc::clone(&started);
    let ctx = Ctx::new(cancel, move |_, what| {
        if what.starts_with("Transcribing") && s2.lock().unwrap().is_none() {
            *s2.lock().unwrap() = Some(std::time::Instant::now());
            if let Some(after) = cancel_after {
                let c3 = c2.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs_f64(after));
                    c3.cancel();
                });
            }
        }
    });
    let r = lyrics::transcribe(&m, &buf.channels, buf.sample_rate, &opts, &ctx);
    if let Some(after) = cancel_after {
        let t = started.lock().unwrap().map_or(0.0, |s| s.elapsed().as_secs_f64());
        println!("cancel requested {after:.1} s into transcription; stopped {:.2} s after the request ({:?})", t - after, r.as_ref().err());
        return;
    }
    let (l, note) = r.unwrap();
    let t = t0.elapsed().as_secs_f64();
    println!(
        "{} ({}, skip_silence {}): {len:.0} s of audio in {t:.1} s ({:.2}x real time) [{note}]",
        m.card.file,
        std::env::var("FREEDAWM_WHISPER_THREADS").or_else(|_| std::env::var("SOUNDCRAFT_WHISPER_THREADS")).unwrap_or_else(|_| "default".into()),
        opts.skip_silence,
        len / t
    );
    for s in &l.segments {
        println!("  [{:7.2}] {}", s.start, s.text);
    }
}
