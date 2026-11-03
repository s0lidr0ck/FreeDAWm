//! Renders DeepFilterNet de-noise on an excerpt of a WAV file at several attenuation limits, as
//! 24-bit WAVs, and measures it on the same excerpt with white noise added (SNR against the
//! excerpt before and after).
//!
//! `cargo run --release -p soundcraft-ml --example denoise_render -- <in.wav> <start s> <dur s> <outdir>`

use soundcraft_ml::Ctx;

fn snr(reference: &[f32], x: &[f32]) -> f64 {
    let (mut s, mut e) = (0.0f64, 0.0f64);
    for (r, v) in reference.iter().zip(x) {
        s += f64::from(*r) * f64::from(*r);
        e += f64::from(r - v) * f64::from(r - v);
    }
    10.0 * (s / e.max(1e-20)).log10()
}

fn write(path: &std::path::Path, ch: Vec<Vec<f32>>, rate: u32) -> Result<(), String> {
    let buf = soundcraft_audio_io::AudioBuffer { sample_rate: rate, channels: ch };
    let bytes = soundcraft_audio_io::encode(&buf, &soundcraft_audio_io::EncodeOptions::default()).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())?;
    println!("wrote {}", path.display());
    Ok(())
}

fn main() -> Result<(), String> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [input, start, dur, out] = a.as_slice() else { return Err("usage: denoise_render <in.wav> <start s> <dur s> <outdir>".into()) };
    let start: f64 = start.parse().map_err(|_| "start")?;
    let dur: f64 = dur.parse().map_err(|_| "dur")?;
    let bytes = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
    let (info, buf) = soundcraft_audio_io::decode(&bytes, Some("wav")).map_err(|e| e.to_string())?;
    let rate = info.sample_rate;
    let a0 = (start * f64::from(rate)) as usize;
    let n = (dur * f64::from(rate)) as usize;
    let excerpt: Vec<Vec<f32>> = buf.channels.iter().map(|c| c.iter().skip(a0).take(n).copied().collect()).collect();
    let audio48 = if rate == 48_000 { excerpt.clone() } else { soundcraft_dsp::offline::resample(&excerpt, rate, 48_000) };
    let out = std::path::Path::new(out);
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let stem = std::path::Path::new(input).file_stem().and_then(|s| s.to_str()).unwrap_or("in").to_string();
    let tag = format!("{stem}_{start:.0}s-{:.0}s", start + dur);
    write(&out.join(format!("{tag}_original.wav")), excerpt.clone(), rate)?;
    for atten in [12.0f32, 24.0, 100.0] {
        let t0 = std::time::Instant::now();
        let y = soundcraft_ml::denoise::denoise_48k(&audio48, atten, &Ctx::default()).map_err(|e| e.to_string())?;
        let secs = t0.elapsed().as_secs_f64();
        println!("atten {atten} dB: {secs:.2} s for {dur:.1} s of audio (RTF {:.3})", secs / dur);
        let y = if rate == 48_000 { y } else { soundcraft_dsp::offline::resample(&y, 48_000, rate) };
        write(&out.join(format!("{tag}_denoise_{atten:.0}dB.wav")), y, rate)?;
    }
    // Measured: the excerpt plus white noise at about -40 dBFS RMS.
    let mut seed = 7u32;
    let noisy: Vec<Vec<f32>> = audio48
        .iter()
        .map(|c| {
            c.iter()
                .map(|v| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    v + ((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.035
                })
                .collect()
        })
        .collect();
    let clean = audio48.first().cloned().unwrap_or_default();
    println!("with added noise: input SNR {:.2} dB", snr(&clean, noisy.first().map(Vec::as_slice).unwrap_or_default()));
    for atten in [12.0f32, 24.0, 100.0] {
        let y = soundcraft_ml::denoise::denoise_48k(&noisy, atten, &Ctx::default()).map_err(|e| e.to_string())?;
        println!("  atten {atten:>5} dB: output SNR {:.2} dB", snr(&clean, y.first().map(Vec::as_slice).unwrap_or_default()));
        if atten >= 100.0 {
            write(&out.join(format!("{tag}_plusnoise.wav")), noisy.clone(), 48_000)?;
            write(&out.join(format!("{tag}_plusnoise_denoise_100dB.wav")), y, 48_000)?;
            let all = soundcraft_ml::denoise::denoise_48k_with(&noisy, atten, false, &Ctx::default()).map_err(|e| e.to_string())?;
            write(&out.join(format!("{tag}_plusnoise_denoise_100dB_allstages.wav")), all, 48_000)?;
        }
    }
    Ok(())
}
