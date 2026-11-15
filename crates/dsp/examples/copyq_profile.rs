//! CopyQ profiles from audio files, and a look inside one.
//!
//! ```sh
//! cargo run --release -p soundcraft-dsp --example copyq_profile -- make snare.wav
//! cargo run --release -p soundcraft-dsp --example copyq_profile -- make recordings/ profiles/ gate=30
//! cargo run --release -p soundcraft-dsp --example copyq_profile -- show profiles/snare.json
//! cargo run --release -p soundcraft-dsp --example copyq_profile -- render in.wav out.wav profiles/snare.json amount=100 detail=40
//! ```
//!
//! - `make <file or folder> [out file or folder] [gate=dB]` measures each audio file (wav, aiff,
//!   flac, mp3, ogg, opus, m4a) into `<name>.json` beside it, or in the folder given. The same
//!   measurement the app makes (`mix.copyq_measure`): silence and bleed left out (`gate`, default
//!   24 dB under the programme level). Put the results in the app's `CopyQ Profiles` folder
//!   (under its preset folder) and they are in every CopyQ's library.
//! - `show <profile.json>` prints a profile: how much was measured and its curve per octave.
//! - `render <in> <out> <profile.json> [k=v ..]` runs a file through CopyQ matching to the
//!   profile and writes the result (24-bit WAV, the latency taken off), to hear a match without
//!   the app. Knobs: any CopyQ parameter id (`amount`, `detail`, `limit`, `phase` …).
//!
//! Only use recordings you may: a profile is a measurement of its source.

use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_dsp::Plugin;
use soundcraft_dsp::plugins::fs::copyq::measure::measure_audio;
use soundcraft_dsp::plugins::fs::copyq::profile::{ALL, LOUD, MID, QUIET, SIDE, SUM, grid_hz, grid_pos};
use soundcraft_dsp::plugins::fs::copyq::{CopyQ, CopyQState, GRID_N, Profile};
use std::path::{Path, PathBuf};

type Res<T> = Result<T, String>;

const AUDIO: [&str; 9] = ["wav", "wave", "aif", "aiff", "flac", "mp3", "ogg", "opus", "m4a"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("make") => make(&args[1..]),
        Some("show") => show(&args[1..]),
        Some("render") => render(&args[1..]),
        _ => Err("usage: copyq_profile make <file|folder> [out] [gate=dB] | show <profile.json> | render <in> <out> <profile.json> [k=v ..]".into()),
    };
    if let Err(e) = r {
        eprintln!("copyq_profile: {e}");
        std::process::exit(1);
    }
}

fn read_audio(path: &Path) -> Res<AudioBuffer> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let ext = path.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
    let (_, buf) = soundcraft_audio_io::decode(&bytes, ext.as_deref()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(buf)
}

fn read_profile(path: &Path) -> Res<Profile> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Profile::from_json(&text).ok_or_else(|| format!("{}: not a CopyQ profile", path.display()))
}

fn is_audio(p: &Path) -> bool {
    p.extension().and_then(|x| x.to_str()).is_some_and(|x| AUDIO.iter().any(|a| a.eq_ignore_ascii_case(x)))
}

fn knobs(args: &[String]) -> (Vec<&String>, Vec<(&str, f32)>) {
    let mut plain = Vec::new();
    let mut kv = Vec::new();
    for a in args {
        match a.split_once('=').and_then(|(k, v)| v.parse::<f32>().ok().map(|v| (k, v))) {
            Some(pair) => kv.push(pair),
            None => plain.push(a),
        }
    }
    (plain, kv)
}

fn make(args: &[String]) -> Res<()> {
    let (plain, kv) = knobs(args);
    let src = PathBuf::from(plain.first().ok_or("make: give a file or a folder")?);
    let gate = kv.iter().find(|(k, _)| *k == "gate").map_or(24.0, |(_, v)| *v);
    let files: Vec<PathBuf> = if src.is_dir() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(&src)
            .map_err(|e| format!("{}: {e}", src.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && is_audio(p))
            .collect();
        v.sort();
        v
    } else {
        vec![src.clone()]
    };
    if files.is_empty() {
        return Err(format!("{}: no audio files", src.display()));
    }
    let out = plain.get(1).map(PathBuf::from);
    let mut made = 0;
    for f in &files {
        let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or("Profile");
        let buf = match read_audio(f) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("skipped: {e}");
                continue;
            }
        };
        let channels: Vec<&[f32]> = buf.channels.iter().map(Vec::as_slice).collect();
        let p = measure_audio(&channels, f64::from(buf.sample_rate), 0, usize::MAX, gate, stem);
        if !p.is_measured() {
            eprintln!("skipped: {}: silent or too short", f.display());
            continue;
        }
        let to = match &out {
            Some(o) if src.is_dir() || o.is_dir() => o.join(format!("{stem}.json")),
            Some(o) => o.clone(),
            None => f.with_extension("json"),
        };
        if let Some(dir) = to.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(&to, p.to_json()).map_err(|e| format!("{}: {e}", to.display()))?;
        println!("{} -> {} ({:.1} s measured, {})", f.display(), to.display(), p.seconds[ALL], if p.stereo { "stereo" } else { "mono" });
        made += 1;
    }
    if made == 0 { Err("nothing was measured".into()) } else { Ok(()) }
}

fn show(args: &[String]) -> Res<()> {
    let p = read_profile(Path::new(args.first().ok_or("show: give a profile")?))?;
    println!("{} ({})", p.name, if p.stereo { "stereo" } else { "mono" });
    for (lv, name) in [(ALL, "all"), (LOUD, "loud"), (QUIET, "quiet")] {
        let used = if p.has_level(lv) { "" } else { "  (not enough to use)" };
        println!("  {name:>5}: {:6.1} s at {:6.1} dB{used}", p.seconds[lv], p.level[lv]);
    }
    println!("  per octave, dB (twelfth-octave power; pink noise is a flat line):");
    println!("  {:>8}  {:>7}  {:>7}  {:>7}  {:>7}", "Hz", "all", "loud", "quiet", "side-mid");
    for hz in [31.5f32, 63.0, 125.0, 250.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 8_000.0, 16_000.0] {
        let i = (grid_pos(hz).round() as usize).min(GRID_N - 1);
        let width = if p.stereo { format!("{:7.1}", p.curve(ALL, SIDE)[i] - p.curve(ALL, MID)[i]) } else { "      -".into() };
        println!("  {:8.0}  {:7.1}  {:7.1}  {:7.1}  {width}", grid_hz(i), p.curve(ALL, SUM)[i], p.curve(LOUD, SUM)[i], p.curve(QUIET, SUM)[i]);
    }
    Ok(())
}

fn render(args: &[String]) -> Res<()> {
    let (plain, kv) = knobs(args);
    let [input, output, profile] = plain.as_slice() else { return Err("render: give <in> <out> <profile.json>".into()) };
    let buf = read_audio(Path::new(input))?;
    let target = read_profile(Path::new(profile))?;
    let nc = buf.channels.len().clamp(1, 2);
    let mut q = CopyQ::new();
    q.prepare(buf.sample_rate as f32, 1024, nc);
    for (k, v) in &kv {
        if !q.set_param(k, *v) {
            return Err(format!("CopyQ has no parameter `{k}`"));
        }
    }
    if !q.load_state(&CopyQState { targets: vec![(1.0, target)], source: None }.write()) {
        return Err("the profile was refused".into());
    }
    let len = buf.channels.iter().take(nc).map(Vec::len).min().unwrap_or(0);
    let pass = |q: &mut CopyQ, keep: bool| -> Vec<Vec<f32>> {
        let lat = q.latency();
        let mut out: Vec<Vec<f32>> = vec![Vec::with_capacity(len + lat); nc];
        let mut pos = 0;
        while pos < len + lat {
            let n = 1024.min(len + lat - pos);
            let mut io: Vec<Vec<f32>> =
                buf.channels.iter().take(nc).map(|c| (pos..pos + n).map(|i| c.get(i).copied().unwrap_or(0.0)).collect()).collect();
            q.process(&mut io, n);
            if keep {
                for (o, b) in out.iter_mut().zip(&io) {
                    o.extend_from_slice(b);
                }
            }
            pos += n;
        }
        out.into_iter().map(|c| c.into_iter().skip(lat).collect()).collect()
    };
    // Once through to learn the file (Follow), then again for the result: the whole file is
    // matched as it ends up, not as it was a second in.
    pass(&mut q, false);
    q.reset();
    let out = pass(&mut q, true);
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int24, ..EncodeOptions::default() };
    let bytes =
        soundcraft_audio_io::encode(&AudioBuffer { sample_rate: buf.sample_rate, channels: out }, &opts).map_err(|e| format!("{output}: {e}"))?;
    std::fs::write(output, bytes).map_err(|e| format!("{output}: {e}"))?;
    let mut r = [f32::NAN; 8];
    q.readouts(&mut r);
    println!("{output}: cut up to {:.1} dB, lift up to {:.1} dB, level {:+.1} dB", r[0], r[1], r[2]);
    Ok(())
}
