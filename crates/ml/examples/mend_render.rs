//! Mend (the live insert on Smule Renaissance) against the whole-file model on a WAV excerpt: how
//! closely the two agree, how long the model takes per block, and 24-bit WAVs of both.
//!
//! `cargo run --release -p soundcraft-ml --example mend_render -- <in.wav> <start s> <dur s> <outdir> [realtime]`
//!
//! Without `realtime` Mend renders offline (it waits for every answer). With it the input is fed
//! at the speed of playback, so the answers that come late are heard as the input.

use soundcraft_dsp::Plugin;
use soundcraft_dsp::plugins::mend::Mend;
use soundcraft_ml::{Ctx, Device};

fn write(path: &std::path::Path, ch: Vec<Vec<f32>>, rate: u32) -> Result<(), String> {
    let buf = soundcraft_audio_io::AudioBuffer { sample_rate: rate, channels: ch };
    let bytes = soundcraft_audio_io::encode(&buf, &soundcraft_audio_io::EncodeOptions::default()).map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())?;
    println!("wrote {}", path.display());
    Ok(())
}

/// How far `est` is from `reference`, as a signal-to-difference ratio in dB.
fn agreement_db(reference: &[f32], est: &[f32]) -> f64 {
    let (mut s, mut d) = (0.0f64, 0.0f64);
    for (a, b) in reference.iter().zip(est) {
        s += f64::from(*a).powi(2);
        d += f64::from(a - b).powi(2);
    }
    10.0 * (s / d.max(1e-30)).log10()
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, start, dur, outdir, rest @ ..] = args.as_slice() else {
        return Err("usage: mend_render <in.wav> <start s> <dur s> <outdir> [realtime]".into());
    };
    let realtime = rest.first().is_some_and(|a| a == "realtime");
    let bytes = std::fs::read(input).map_err(|e| e.to_string())?;
    let (_, buf) = soundcraft_audio_io::decode(&bytes, None).map_err(|e| e.to_string())?;
    let rate = buf.sample_rate;
    let (a, n) = (
        (start.parse::<f64>().map_err(|e| e.to_string())? * f64::from(rate)) as usize,
        (dur.parse::<f64>().map_err(|e| e.to_string())? * f64::from(rate)) as usize,
    );
    let mono: Vec<f32> = buf.channels.first().map(|c| c.iter().skip(a).take(n).copied().collect()).unwrap_or_default();
    let outdir = std::path::Path::new(outdir);
    std::fs::create_dir_all(outdir).map_err(|e| e.to_string())?;

    // The whole-file model, as Clean Vocal runs it (at 48 kHz).
    let dir = soundcraft_ml::models::models_dir();
    let model = soundcraft_ml::models::load(&dir, soundcraft_ml::srs::ID).map_err(|e| e.to_string())?;
    let at48 = soundcraft_dsp::offline::resample(std::slice::from_ref(&mono), rate, 48_000);
    let t = std::time::Instant::now();
    let (stems, device) = soundcraft_ml::srs::run(&model, &at48, Device::Auto, &Ctx::default()).map_err(|e| e.to_string())?;
    println!("whole file: {:.2} s for {:.1} s of sound on the {device}", t.elapsed().as_secs_f32(), mono.len() as f32 / rate as f32);
    let mut whole48 = stems.into_iter().next().map(|(_, a)| a).unwrap_or_default();
    // Clean Vocal's make-up: one gain that brings the result to the input's loudness.
    let makeup = soundcraft_ml::clean::makeup_gain(&at48, &whole48, 48_000);
    soundcraft_ml::clean::apply_gain(&mut whole48, makeup);
    let whole = soundcraft_dsp::offline::resample(&whole48, 48_000, rate).into_iter().next().unwrap_or_default();

    // Mend.
    soundcraft_ml::live::start();
    let mut mend = Mend::new();
    mend.set_offline(!realtime);
    mend.prepare(rate as f32, 512, 2);
    let lat = mend.latency();
    let mut out = Vec::with_capacity(mono.len() + lat);
    let t = std::time::Instant::now();
    let padded: Vec<f32> = mono.iter().copied().chain(std::iter::repeat_n(0.0, lat)).collect();
    let mut readouts = [f32::NAN; 8];
    let (mut worst_load, mut played) = (0.0f32, 0usize);
    for block in padded.chunks(512) {
        let mut io = vec![block.to_vec(), block.to_vec()];
        mend.process(&mut io, block.len());
        out.extend_from_slice(&io[0]);
        mend.readouts(&mut readouts);
        worst_load = worst_load.max(readouts[1]);
        played += block.len();
        if realtime {
            let due = std::time::Duration::from_secs_f64(played as f64 / f64::from(rate));
            if let Some(wait) = due.checked_sub(t.elapsed()) {
                std::thread::sleep(wait);
            }
        }
    }
    let mended: Vec<f32> = out.split_off(lat.min(out.len()));
    println!(
        "Mend ({}): latency {:.2} s, {:.2} s to render, state {}, worst block {:.0} % of real time, late {} time(s)",
        if realtime { "realtime" } else { "offline" },
        lat as f32 / rate as f32,
        t.elapsed().as_secs_f32(),
        readouts[0],
        worst_load,
        readouts[4]
    );
    let n = mended.len().min(whole.len());
    println!("Mend against the whole-file model: {:.1} dB", agreement_db(&whole[..n], &mended[..n]));
    println!("input against the whole-file model: {:.1} dB", agreement_db(&whole[..n], &mono[..n]));
    write(&outdir.join("input.wav"), vec![mono], rate)?;
    write(&outdir.join("whole_file.wav"), vec![whole], rate)?;
    write(&outdir.join("mend.wav"), vec![mended], rate)?;
    Ok(())
}
