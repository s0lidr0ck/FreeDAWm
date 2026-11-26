//! Renders audio through the real Plumb module (`soundcraft_dsp`'s `autopitch`), block by
//! block as the mixer runs it, for listening, and measures its CPU load.
//!
//! `cargo run --release -p soundcraft-vocal --example autopitch_render -- <out_dir>`: the study's
//! synthetic phrase (generated, no recording) at each preset, `synth_<preset>.wav`, with the dry
//! input; and the CPU share of one core for a stereo voice at 48 kHz.
//!
//! `... -- <out_dir> <take.wav> <from s> <to s> <key 1..12 (1 = C)> [scale index] [live]`: that
//! stretch of a recording at each preset, `take_<preset>.wav` with `take_dry.wav` (with `live`:
//! in Live mode, Middle voice range, `take_live_<preset>.wav`). `... -- <out_dir> live`: the
//! synthetic phrase in Live mode too. Nothing is read or
//! written anywhere else; the files are for the person whose take it is.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../tests/autopitch_fixtures/mod.rs"]
mod fixtures;

use soundcraft_audio_io::{AudioBuffer, EncodeOptions, decode, encode};

fn write(path: &str, sr: u32, ch: &[Vec<f32>]) {
    let buf = AudioBuffer { sample_rate: sr, channels: ch.to_vec() };
    std::fs::write(path, encode(&buf, &EncodeOptions::default()).unwrap()).unwrap();
}

/// The study's phrase: the gospel line, slides, the fast run, the low and the high lines, with
/// short gaps.
fn phrase() -> Vec<f32> {
    let mut out = Vec::new();
    for name in ["gospel", "slides", "melisma", "low_male", "high_female"] {
        let f = fixtures::by_name(name).unwrap();
        out.extend_from_slice(&f.x[0]);
        out.extend(std::iter::repeat_n(0.0, 12_000));
    }
    out
}

/// The share of the voiced time (the Vocal Editor's tracker on the mono sum) within 15 cents of
/// a note in `mask`.
fn in_key(x: &[Vec<f32>], sr: u32, mask: u16) -> f64 {
    let n = x[0].len();
    let mono: Vec<f32> = (0..n).map(|i| x.iter().map(|c| c[i]).sum::<f32>() / x.len() as f32).collect();
    let t = soundcraft_vocal::track_pitch(&mono, sr, &std::sync::atomic::AtomicBool::new(false)).unwrap();
    let (mut v, mut ok) = (0usize, 0usize);
    for c in t.frames.iter().filter_map(|f| f.cents) {
        v += 1;
        let m = (f64::from(c) / 100.0).round() as i64;
        let near = (m - 1..=m + 1).filter(|k| mask & (1 << k.rem_euclid(12)) != 0).any(|k| (f64::from(c) - k as f64 * 100.0).abs() <= 15.0);
        ok += usize::from(near);
    }
    100.0 * ok as f64 / v.max(1) as f64
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args.first().cloned().unwrap_or_else(|| ".".into());
    std::fs::create_dir_all(&out).unwrap();
    if args.len() >= 5 {
        let bytes = std::fs::read(&args[1]).unwrap();
        let (_, buf) = decode(&bytes, Some("wav")).unwrap();
        let sr = buf.sample_rate;
        let (a, b) = (args[2].parse::<f64>().unwrap(), args[3].parse::<f64>().unwrap());
        let (a, b) = ((a * f64::from(sr)) as usize, (b * f64::from(sr)) as usize);
        let x: Vec<Vec<f32>> = buf.channels.iter().map(|c| c[a.min(c.len())..b.min(c.len())].to_vec()).collect();
        let key: f32 = args[4].parse().unwrap();
        let scale: f32 = args.get(5).map_or(0.0, |s| s.parse().unwrap());
        let live = args.get(6).is_some_and(|a| a == "live");
        let tag = if live { "live_" } else { "" };
        write(&format!("{out}/take_dry.wav"), sr, &x);
        let mask = soundcraft_dsp::plugins::autopitch::rotate(
            soundcraft_dsp::plugins::autopitch::SCALES[scale as usize % 12].1,
            (key as u8).saturating_sub(1),
        );
        println!("take dry: {:.0} % of the voiced time within 15 c of a key note", in_key(&x, sr, mask));
        for preset in fixtures::SETTINGS {
            let mut set = fixtures::setting(preset);
            set.push(("key", key));
            set.push(("scale", scale));
            if live {
                set.extend([("mode", 1.0), ("range", 1.0)]);
            }
            let (y, _, d, secs) = fixtures::run_with(&x, &set, 256, sr as f32);
            write(&format!("{out}/take_{tag}{}.wav", preset.to_lowercase()), sr, &y);
            let audio = x[0].len() as f64 / f64::from(sr);
            println!(
                "take {tag}{preset}: latency {d} samples, {:.1} % of one core ({} channels), {:.0} % of the voiced time within 15 c of a key note",
                100.0 * secs / audio,
                x.len(),
                in_key(&y, sr, mask)
            );
        }
        return;
    }
    let x = vec![phrase()];
    write(&format!("{out}/synth_dry.wav"), fixtures::SR, &x);
    let live = args.get(1).is_some_and(|a| a == "live");
    for preset in fixtures::SETTINGS {
        let mut set = fixtures::setting(preset);
        // Live: the Low voice range (the phrase goes down to E2).
        if live {
            set.extend([("mode", 1.0), ("range", 0.0)]);
        }
        let tag = if live { "live_" } else { "" };
        let (y, _, d, _) = fixtures::run(&x, &set);
        write(&format!("{out}/synth_{tag}{}.wav", preset.to_lowercase()), fixtures::SR, &y);
        println!("synth {tag}{preset}: latency {d} samples ({:.1} ms)", d as f64 / 48.0);
    }
    if live {
        let f = fixtures::by_name("stereo_2mic").unwrap();
        let long: Vec<Vec<f32>> = f.x.iter().map(|c| c.repeat(10)).collect();
        let audio = long[0].len() as f64 / fixtures::SRF;
        for preset in fixtures::SETTINGS {
            let mut set = fixtures::setting(preset);
            set.extend([("mode", 1.0), ("range", 1.0)]);
            let (_, _, _, secs) = fixtures::run_with(&long, &set, 128, 48_000.0);
            println!("CPU Live {preset}: {:.2} % of one core, stereo 48 kHz, 128-sample blocks", 100.0 * secs / audio);
        }
        return;
    }
    // CPU: the two-microphone voice, stereo 48 kHz, ten times over, 256-sample blocks.
    let f = fixtures::by_name("stereo_2mic").unwrap();
    let long: Vec<Vec<f32>> = f.x.iter().map(|c| c.repeat(10)).collect();
    let audio = long[0].len() as f64 / fixtures::SRF;
    for preset in fixtures::SETTINGS {
        let (_, _, _, secs) = fixtures::run_with(&long, &fixtures::setting(preset), 256, 48_000.0);
        println!("CPU {preset}: {:.2} % of one core, stereo 48 kHz ({audio:.0} s of audio in {secs:.2} s)", 100.0 * secs / audio);
    }
    let silent = vec![vec![0.0f32; long[0].len()]; 2];
    let (_, _, _, secs) = fixtures::run_with(&silent, &fixtures::setting("Gentle"), 256, 48_000.0);
    println!("CPU silence: {:.3} % of one core", 100.0 * secs / audio);
}
