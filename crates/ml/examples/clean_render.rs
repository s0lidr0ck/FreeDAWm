//! Clean Vocal on an excerpt of a WAV file in both modes: per-20 ms gain statistics on the voiced
//! parts (mean, sd, jumps over 3 dB), the make-up gain, Renaissance's alignment with its input
//! (cross-correlation lag, per-band phase coherence) and 24-bit WAVs of each result.
//!
//! `cargo run --release -p soundcraft-ml --example clean_render -- <in.wav> <start s> <dur s> <outdir>`

use soundcraft_ml::clean::{CleanOptions, level_stats};
use soundcraft_ml::{Ctx, Device, Job, JobKind};

fn write(path: &std::path::Path, ch: Vec<Vec<f32>>, rate: u32) -> Result<(), String> {
    let buf = soundcraft_audio_io::AudioBuffer { sample_rate: rate, channels: ch };
    let bytes = soundcraft_audio_io::encode(&buf, &soundcraft_audio_io::EncodeOptions::default()).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())?;
    println!("wrote {}", path.display());
    Ok(())
}

/// The lag (samples) of the peak normalised cross-correlation of `y` against `x`, within ±`max`.
fn best_lag(x: &[f32], y: &[f32], max: i64) -> (i64, f64) {
    let n = x.len().min(y.len()) as i64;
    let mut best = (0i64, f64::MIN);
    for lag in -max..=max {
        let (mut s, mut ex, mut ey) = (0.0f64, 0.0f64, 0.0f64);
        let mut i = max;
        while i < n - max {
            let a = f64::from(x[i as usize]);
            let b = f64::from(y[(i + lag) as usize]);
            s += a * b;
            ex += a * a;
            ey += b * b;
            i += 1;
        }
        let r = s / (ex * ey).sqrt().max(1e-30);
        if r > best.1 {
            best = (lag, r);
        }
    }
    best
}

/// Magnitude-squared-free phase coherence per band: |Σ X·Y*| / sqrt(Σ|X|² Σ|Y|²) over frames.
fn coherence(x: &[f32], y: &[f32], rate: u32, bands: &[(f32, f32)]) -> Vec<(f64, f64)> {
    let stft = soundcraft_ml::stft::Stft::new(2048, 512);
    let (sx, sy) = (stft.forward(x), stft.forward(y));
    let hz = rate as f32 / 2048.0;
    bands
        .iter()
        .map(|&(lo, hi)| {
            let (mut re, mut im, mut ex, mut ey) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
            for (f, (rx, ry)) in sx.iter().zip(&sy).enumerate() {
                let fh = f as f32 * hz;
                if fh < lo || fh >= hi {
                    continue;
                }
                for (a, b) in rx.iter().zip(ry) {
                    let p = a * b.conj();
                    re += f64::from(p.re);
                    im += f64::from(p.im);
                    ex += f64::from(a.norm_sqr());
                    ey += f64::from(b.norm_sqr());
                }
            }
            // The cross-spectrum's angle at the band centre, as a delay in samples (y after x > 0).
            let fc = f64::from((lo + hi) / 2.0);
            let delay = -im.atan2(re) / (2.0 * std::f64::consts::PI * fc) * f64::from(rate);
            ((re * re + im * im).sqrt() / (ex * ey).sqrt().max(1e-30), -delay)
        })
        .collect()
}

fn main() -> Result<(), String> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [input, start, dur, out] = a.as_slice() else { return Err("usage: clean_render <in.wav> <start s> <dur s> <outdir>".into()) };
    let start: f64 = start.parse().map_err(|_| "start")?;
    let dur: f64 = dur.parse().map_err(|_| "dur")?;
    let bytes = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
    let (info, buf) = soundcraft_audio_io::decode(&bytes, Some("wav")).map_err(|e| e.to_string())?;
    let rate = info.sample_rate;
    let a0 = (start * f64::from(rate)) as usize;
    let n = (dur * f64::from(rate)) as usize;
    let excerpt: Vec<Vec<f32>> = buf.channels.iter().map(|c| c.iter().skip(a0).take(n).copied().collect()).collect();
    drop(buf);
    println!("{input}: {} ch at {rate} Hz, {start} s + {dur} s", excerpt.len());
    let out = std::path::Path::new(out);
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let stem = std::path::Path::new(input).file_stem().and_then(|s| s.to_str()).unwrap_or("in").to_string();
    let tag = format!("{stem}_{start:.0}s-{:.0}s", start + dur);
    write(&out.join(format!("{tag}_original.wav")), excerpt.clone(), rate)?;
    let models_dir = soundcraft_ml::models::models_dir();
    let runs: [(&str, CleanOptions); 3] = [
        ("vocal", CleanOptions::vocal(1.0)),
        ("speech", CleanOptions::speech(24.0, true)),
        ("speech-denoise-only", CleanOptions::speech(24.0, false)),
    ];
    let mut vocal = None;
    for (name, opts) in runs {
        let job = Job {
            kind: JobKind::Clean(opts),
            audio: excerpt.clone(),
            rate,
            device: Device::Auto,
            models_dir: models_dir.clone(),
            quality: soundcraft_ml::Quality::Fast,
        };
        let t0 = std::time::Instant::now();
        let res = soundcraft_ml::pipeline::run(&job, &Ctx::default()).map_err(|e| e.to_string())?;
        let secs = t0.elapsed().as_secs_f64();
        let y = res.stems.into_iter().next().map(|s| s.audio).ok_or("no output")?;
        for range in [12.0, 20.0] {
            let st = level_stats(&excerpt, &y, rate, range);
            println!(
                "{name:<20} windows within {range} dB of the loudest: {}  gain mean {:+.2} dB  sd {:.2} dB  jumps {}",
                st.windows, st.mean_db, st.sd_db, st.jumps
            );
        }
        let st = level_stats(&excerpt, &y, rate, 30.0);
        println!(
            "{name:<20} {secs:6.1} s  [{}]  voiced windows {}  gain mean {:+.2} dB  sd {:.2} dB  jumps>3dB {}  gated loudness out-in {:+.2} dB",
            res.notes.join("; "),
            st.windows,
            st.mean_db,
            st.sd_db,
            st.jumps,
            st.loudness_diff_db
        );
        write(&out.join(format!("{tag}_{name}.wav")), y.clone(), rate)?;
        if name == "vocal" {
            vocal = Some(y);
        }
    }
    // Alignment of Renaissance's output with its input (first channel, 5 s from 10 s in).
    if let Some(y) = vocal {
        let (x0, y0) = (excerpt.first().cloned().unwrap_or_default(), y.first().cloned().unwrap_or_default());
        let s0 = (10 * rate) as usize;
        let e0 = (s0 + 5 * rate as usize).min(x0.len());
        let (xs, ys) = (x0.get(s0..e0).unwrap_or_default(), y0.get(s0..e0).unwrap_or_default());
        let (lag, r) = best_lag(xs, ys, 2048);
        println!("alignment: cross-correlation peak at lag {lag} samples (r = {r:.3}); r at lag 0 = {:.3}", best_lag(xs, ys, 0).1);
        let bands = [(80.0, 300.0), (300.0, 1000.0), (1000.0, 3000.0), (3000.0, 8000.0), (8000.0, 16000.0)];
        let coh = coherence(x0.as_slice(), y0.as_slice(), rate, &bands);
        for ((lo, hi), (c, d)) in bands.iter().zip(coh) {
            println!("  phase coherence {lo:>5.0}-{hi:<5.0} Hz: {c:.3} (mean phase as delay {d:+.2} samples)");
        }
        // The 50 % blend, as the clip plays it.
        let half: Vec<Vec<f32>> = excerpt.iter().zip(&y).map(|(a, b)| a.iter().zip(b).map(|(p, q)| 0.5 * p + 0.5 * q).collect()).collect();
        let st = level_stats(&excerpt, &half, rate, 30.0);
        println!("vocal 50 % mix       gain mean {:+.2} dB  sd {:.2} dB  jumps {}", st.mean_db, st.sd_db, st.jumps);
        write(&out.join(format!("{tag}_vocal_mix50.wav")), half, rate)?;
    }
    Ok(())
}
