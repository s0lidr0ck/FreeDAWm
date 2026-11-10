//! `freedawm-cli`: FreeDAWm without a window.
//!
//! ```text
//! freedawm-cli info FILE                       audio file or .fdawm summary
//! freedawm-cli convert IN OUT [--bit-depth 16|24|32f] [--rate HZ] [--normalize]
//! freedawm-cli run [--in S.fdawm | --demo] [--cmd ID[=JSON]]… [--bounce OUT] [--start T --end T] [--save S.fdawm] [--inspect]
//! freedawm-cli script FILE [--in S | --demo] [--save S] [--bounce OUT]   (one `ID JSON` per line)
//! freedawm-cli commands [FILTER]               list commands
//! freedawm-cli describe ID                     one command's details
//! freedawm-cli app [--port P] METHOD [JSON]     call a running app's control channel
//! freedawm-cli mcp [--connect PORT] [--demo] [--automation-read-root DIR] [--automation-write-root DIR]
//!                                                 MCP server on stdio. The two roots confine every file the
//!                                                 agent's commands read (open, import, presets) and write
//!                                                 (save, bounce, export) to those folders, links followed;
//!                                                 a root left out grants none of its access. They run the
//!                                                 in-process server, so they conflict with --connect.
//! freedawm-cli parity [--write PATH]            parity report (markdown)
//! freedawm-cli plugins                          list built-in plugins
//! freedawm-cli ml separate|karaoke|drums|clean IN.wav OUTDIR [--device auto|gpu|cpu] [--models DIR]
//!                 [--model demucs|sw6|best] [--keep-backing] [--from-mix] [--quality fast|high] [--mode vocal|speech] [--mix PCT] [--amount DB] [--no-dereverb] [--no-denoise] [--bit-depth 16|24|32f]
//!                                                 local ML on a file: writes OUTDIR/<in> - <Stem>.wav (32-bit float)
//! freedawm-cli ml midi IN.wav OUT.mid [--model general|piano|drums|vocal] [--onset 0..1] [--frame 0..1]
//!                 [--min-ms MS] [--bends] [--no-pedal] [--isolate none|drums|piano|other|vocals|bass] [--quality fast|high] [--bpm 120] [--device auto|gpu|cpu] [--models DIR]
//!                                                 audio to MIDI: a format-1 .mid at a constant --bpm
//! freedawm-cli ml chords IN.wav OUT.json|OUT.lab [--bass | --no-bass] [--device auto|gpu|cpu] [--models DIR]
//!                                                 chord recognition (BTC): segments with times and names;
//!                                                 the bass guess (inversions) defaults on for a stereo file
//! freedawm-cli ml lyrics IN.wav OUT.lrc|OUT.txt|OUT.json [--language en|auto] [--prompt TEXT] [--quality fast|best]
//!                 [--skip-silence] [--models DIR]
//!                                                 sung lyrics (Whisper large-v3, CPU) with line and word times
//! freedawm-cli ml text-midi OUT.mid --prompt TEXT [--seconds 30] [--seed N] [--temperature 1] [--bpm 120]
//!                 [--device auto|gpu|cpu] [--models DIR]
//!                                                 text to MIDI (MIDI-LLM): a format-1 .mid, one track per
//!                                                 General MIDI instrument, at a constant --bpm
//! freedawm-cli ml models [--models DIR]         the model folder and which models are installed
//! freedawm-cli ml bench IN.wav [--seconds 30]   HTDemucs speed on the GPU (DirectML) and the CPU
//! ```
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod automation_args;

use serde_json::{Value, json};
use soundcraft_automation::{Backend, Headless, Remote, mcp::Server};
use soundcraft_engine::Engine;
use soundcraft_engine::file_access::{self, AutomationRoots};
use std::io::Write;
use std::process::ExitCode;

/// Print without panicking on a closed pipe. A write error must not skip later work
/// such as `--bounce` or `--save`, and must not by itself report success.
macro_rules! outln {
    ($($t:tt)*) => {{
        let mut o = std::io::stdout().lock();
        let _ = writeln!(o, $($t)*);
    }};
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn fail(msg: impl std::fmt::Display) -> ExitCode {
    eprintln!("freedawm-cli: {msg}");
    ExitCode::FAILURE
}

/// The usage block of the module doc comment, up to its closing code fence.
fn help_text() -> String {
    include_str!("main.rs")
        .lines()
        .skip(3)
        .take_while(|l| l.trim_end() != "//! ```")
        .map(|l| l.trim_start_matches("//! "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The plugin scan cache the app keeps too (`<config>/PluginScan`): `$FREEDAWM_CONFIG_DIR` (or the
/// older `$SOUNDCRAFT_CONFIG_DIR`), else the platform's settings folder. That folder is copied
/// from the pre-rename `SoundCraft` folder when it has not been yet (as the app does; one line on
/// standard error says what was copied or what failed). None with `FREEDAWM_NO_PREFS` (agents'
/// test runs) or an empty override.
fn plugin_scan_dir() -> Option<std::path::PathBuf> {
    use soundcraft_engine::brand;
    if brand::env_var_os("NO_PREFS").is_some() {
        return None;
    }
    let config = match brand::env_var_os("CONFIG_DIR") {
        Some(d) => (!d.is_empty()).then(|| std::path::PathBuf::from(d)),
        None => {
            let (dir, legacy) = brand::profile_settings_dirs(brand::Platform::current(), |k| std::env::var_os(k))?;
            let result = brand::migrate_folder(&legacy, &dir);
            if let Some((_, line)) = brand::migration_summary(&legacy, &dir, &result) {
                eprintln!("freedawm-cli: {line}");
            }
            Some(dir)
        }
    };
    config.map(|c| c.join("PluginScan"))
}

/// Points the plugin hosts at the scan cache; called by the subcommands that open sessions (so
/// `--version`, `--help` and the listings never touch, or copy, the settings folder).
fn use_plugin_scan_cache() {
    soundcraft_engine::set_plugin_scan_cache_dir(plugin_scan_dir());
}

/// A blocking plugin scan's progress, on standard error.
fn scan_progress(done: usize, total: usize) {
    eprintln!("scanning plugins for this session: {done} / {total}");
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().cloned() else {
        eprintln!("usage: freedawm-cli <info|convert|run|script|commands|describe|app|mcp|parity|plugins|ml> …  (see --help)");
        return ExitCode::FAILURE;
    };
    let rest: Vec<String> = args.iter().skip(1).cloned().collect();
    // No UI to keep responsive: a plugin missing from the scan cache is scanned for at once
    // (through the scanner helper), with the progress on standard error.
    soundcraft_engine::set_blocking_plugin_lookups(Some(scan_progress));
    let code = match cmd.as_str() {
        "--version" | "-V" | "version" => {
            outln!("FreeDAWm CLI {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        "--help" | "-h" | "help" => {
            outln!("{}", help_text());
            ExitCode::SUCCESS
        }
        "info" => {
            use_plugin_scan_cache();
            info(&rest)
        }
        "convert" => convert(&rest),
        "run" => {
            use_plugin_scan_cache();
            run(&rest)
        }
        "script" => {
            use_plugin_scan_cache();
            script(&rest)
        }
        "commands" => commands(&rest),
        "describe" => describe(&rest),
        "app" => app(&rest),
        "mcp" => mcp(&rest),
        "parity" => parity(&rest),
        "ml" => ml(&rest),
        "plugins" => {
            for p in soundcraft_dsp::plugins() {
                outln!("{:<20} {:<22} {:?}{}", p.id, p.name, p.category, if p.is_instrument { " (instrument)" } else { "" });
            }
            ExitCode::SUCCESS
        }
        other => fail(format!("unknown subcommand `{other}`")),
    };
    // The subcommand's engine and its plugin instances are gone by now.
    soundcraft_engine::shutdown_plugin_hosts();
    code
}

/// `ml …`: local ML on audio files (see the usage block).
fn ml(args: &[String]) -> ExitCode {
    use soundcraft_ml::{Cancel, Ctx, Device, JobKind};
    let Some(sub) = args.first().map(String::as_str) else {
        return fail("ml separate|karaoke|drums|clean|midi|chords|lyrics|text-midi|models|bench …");
    };
    let dir = arg_value(args, "--models").map_or_else(soundcraft_ml::models::models_dir, std::path::PathBuf::from);
    if sub == "models" {
        outln!("model folder: {}  (override: {})", dir.display(), soundcraft_ml::models::MODELS_DIR_ENV);
        for m in soundcraft_ml::models::list(&dir) {
            let status = match &m.status {
                soundcraft_ml::ModelStatus::Installed(c) => {
                    format!("installed ({:.0} MB) {}", m.size as f64 / 1e6, c.license.lines().next().unwrap_or(""))
                }
                soundcraft_ml::ModelStatus::Builtin => "built in".into(),
                soundcraft_ml::ModelStatus::Missing => "not installed: cargo xtask ml-models".into(),
                soundcraft_ml::ModelStatus::Broken(w) => format!("broken: {w}"),
            };
            outln!("{:<22} {:<34} {status}  [licence: {}]", m.model.id, m.model.name, m.licence());
        }
        return ExitCode::SUCCESS;
    }
    let device = match arg_value(args, "--device").as_deref().map(Device::parse) {
        None => Device::Auto,
        Some(Some(d)) => d,
        Some(None) => return fail("--device must be auto, gpu or cpu"),
    };
    if sub == "text-midi" {
        return ml_text_midi(args, device, &dir);
    }
    let Some(input) = args.get(1) else { return fail(format!("ml {sub} IN.wav …")) };
    let bytes = match std::fs::read(input) {
        Ok(b) => b,
        Err(e) => return fail(format!("{input}: {e}")),
    };
    let ext = std::path::Path::new(input).extension().and_then(|x| x.to_str());
    let (_, buf) = match soundcraft_audio_io::decode(&bytes, ext) {
        Ok(x) => x,
        Err(e) => return fail(format!("{input}: {e}")),
    };
    let progress = |f: f32, what: &str| {
        if !what.is_empty() {
            eprintln!("  {what}");
        }
        let _ = f;
    };
    if sub == "bench" {
        let secs: f64 = arg_value(args, "--seconds").and_then(|s| s.parse().ok()).filter(|s: &f64| *s > 0.0).unwrap_or(30.0);
        let n = (secs * f64::from(buf.sample_rate)) as usize;
        let audio: Vec<Vec<f32>> = buf.channels.iter().map(|c| c.iter().take(n).copied().collect()).collect();
        let len = audio.first().map_or(0, Vec::len) as f64 / f64::from(buf.sample_rate.max(1));
        for device in [Device::Gpu, Device::Cpu] {
            let job = soundcraft_ml::Job {
                kind: JobKind::Separate { model: soundcraft_ml::SepModel::Demucs },
                audio: audio.clone(),
                rate: buf.sample_rate,
                device,
                models_dir: dir.clone(),
                quality: soundcraft_ml::Quality::Fast,
            };
            let t0 = std::time::Instant::now();
            match soundcraft_ml::pipeline::run(&job, &Ctx::new(Cancel::new(), |_, _| {})) {
                Ok(out) => {
                    let t = t0.elapsed().as_secs_f64();
                    outln!(
                        "htdemucs {:<4} {len:.1} s of audio in {t:.2} s ({:.1}x real time) [{}]",
                        device.id(),
                        len / t.max(1e-9),
                        out.notes.join("; ")
                    );
                }
                Err(e) => outln!("htdemucs {:<4} failed: {e}", device.id()),
            }
        }
        return ExitCode::SUCCESS;
    }
    if sub == "midi" {
        let Some(out) = args.get(2) else { return fail("ml midi IN.wav OUT.mid …") };
        return ml_midi(args, buf, out, device, &dir);
    }
    if sub == "chords" {
        let Some(out) = args.get(2) else { return fail("ml chords IN.wav OUT.json|OUT.lab …") };
        return ml_chords(args, buf, out, device, dir);
    }
    if sub == "lyrics" {
        let Some(out) = args.get(2) else { return fail("ml lyrics IN.wav OUT.lrc|OUT.txt|OUT.json …") };
        return ml_lyrics(args, buf, out, &dir);
    }
    let Some(outdir) = args.get(2) else { return fail(format!("ml {sub} IN.wav OUTDIR …")) };
    let flag = |f: &str| args.iter().any(|a| a == f);
    let kind = match sub {
        "separate" => match arg_value(args, "--model").map(|m| soundcraft_ml::SepModel::parse(&m).ok_or(m)) {
            None => JobKind::Separate { model: soundcraft_ml::SepModel::Demucs },
            Some(Ok(model)) => JobKind::Separate { model },
            Some(Err(m)) => return fail(format!("unknown --model `{m}` (demucs, sw6, best)")),
        },
        "karaoke" => JobKind::Karaoke { keep_backing: flag("--keep-backing") },
        "drums" => JobKind::Drums { from_mix: flag("--from-mix") },
        "clean" => {
            let atten_db = arg_value(args, "--amount").and_then(|s| s.parse().ok()).unwrap_or(soundcraft_ml::denoise::DEFAULT_ATTEN_DB);
            let mix = match arg_value(args, "--mix").map(|s| s.parse::<f32>()) {
                None => 1.0,
                Some(Ok(m)) if (0.0..=100.0).contains(&m) => m / 100.0,
                Some(_) => return fail("--mix must be a number from 0 to 100 (%)"),
            };
            let opts = match arg_value(args, "--mode") {
                Some(m) => match soundcraft_ml::CleanMode::parse(&m) {
                    Some(soundcraft_ml::CleanMode::Vocal) => soundcraft_ml::CleanOptions::vocal(mix),
                    Some(soundcraft_ml::CleanMode::Speech) => soundcraft_ml::CleanOptions::speech(atten_db, !flag("--no-dereverb")),
                    None => return fail("--mode must be vocal or speech"),
                },
                // The old switches: --no-dereverb alone means Speech without de-reverb.
                None => match soundcraft_ml::CleanOptions::from_legacy(!flag("--no-denoise"), !flag("--no-dereverb"), atten_db) {
                    Some(o) if o.mode == soundcraft_ml::CleanMode::Vocal => soundcraft_ml::CleanOptions::vocal(mix),
                    Some(o) => o,
                    None => return fail("nothing to do: --no-denoise with --no-dereverb"),
                },
            };
            JobKind::Clean(opts)
        }
        other => return fail(format!("unknown ml job `{other}` (separate, karaoke, drums, clean, midi, chords, lyrics, models, bench)")),
    };
    let bit_depth = match parse_bit_depth(arg_value(args, "--bit-depth").as_deref().or(Some("32f"))) {
        Ok(b) => b,
        Err(e) => return fail(e),
    };
    let quality = match arg_value(args, "--quality") {
        None => soundcraft_ml::Quality::Fast,
        Some(q) => match soundcraft_ml::Quality::parse(&q) {
            Some(q) => q,
            None => return fail("--quality must be fast or high"),
        },
    };
    let job = soundcraft_ml::Job { kind, audio: buf.channels, rate: buf.sample_rate, device, models_dir: dir, quality };
    let t0 = std::time::Instant::now();
    let out = match soundcraft_ml::pipeline::run(&job, &Ctx::new(Cancel::new(), progress)) {
        Ok(o) => o,
        Err(e) => return fail(e),
    };
    let secs = t0.elapsed().as_secs_f64();
    if let Err(e) = std::fs::create_dir_all(outdir) {
        return fail(format!("{outdir}: {e}"));
    }
    let stem = std::path::Path::new(input).file_stem().and_then(|s| s.to_str()).unwrap_or("audio");
    let opts =
        soundcraft_audio_io::EncodeOptions { format: soundcraft_audio_io::FileFormat::Wav, bit_depth, dither: false, bwf: None, bitrate_kbps: 0 };
    for s in out.stems {
        let path = std::path::Path::new(outdir).join(format!("{stem} - {}.wav", s.name));
        let b = soundcraft_audio_io::AudioBuffer { sample_rate: job.rate, channels: s.audio };
        match soundcraft_audio_io::encode(&b, &opts)
            .map_err(|e| e.to_string())
            .and_then(|bytes| std::fs::write(&path, bytes).map_err(|e| e.to_string()))
        {
            Ok(()) => outln!("wrote {}", path.display()),
            Err(e) => return fail(format!("{}: {e}", path.display())),
        }
    }
    outln!("{} in {secs:.2} s ({})", kind.title(), out.notes.join("; "));
    ExitCode::SUCCESS
}

/// `ml midi`: the file as a clip at 0 in a session at `--bpm`, Convert to MIDI on it (the same
/// command the app runs), and the new track exported as a .mid.
fn ml_midi(args: &[String], buf: soundcraft_audio_io::AudioBuffer, out: &str, device: soundcraft_ml::Device, dir: &std::path::Path) -> ExitCode {
    use soundcraft_model::{ChannelFormat, Clip, Session, TrackKind};
    let bpm: f64 = match arg_value(args, "--bpm").map(|s| s.parse::<f64>()) {
        None => 120.0,
        Some(Ok(b)) if b.is_finite() && (20.0..=999.0).contains(&b) => b,
        Some(_) => return fail("--bpm must be a number from 20 to 999"),
    };
    let Ok(rate) = soundcraft_time::SampleRate::new(buf.sample_rate) else { return fail(format!("unsupported sample rate {} Hz", buf.sample_rate)) };
    let mut s = Session { sample_rate: rate, ..Session::default() };
    if let Err(e) = s.tempo.set_tempo(0, bpm) {
        return fail(e);
    }
    let len = buf.channels.first().map_or(0, Vec::len) as i64;
    if len == 0 {
        return fail("the file holds no audio");
    }
    let format = if buf.channels.len() >= 2 { ChannelFormat::Stereo } else { ChannelFormat::Mono };
    let t = s.add_track(TrackKind::Audio, format, Some("Audio"));
    let src = soundcraft_engine::io::add_source(&mut s, "Audio", buf, None, soundcraft_audio_io::FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(id, "Audio", src, 0, 0, len));
    let mut e = Engine::new(s);
    let mut p = json!({"track": t.0, "device": device.id(), "models_dir": dir.to_string_lossy(),
        "model": arg_value(args, "--model").unwrap_or_else(|| "general".into()),
        "pitch_bends": args.iter().any(|a| a == "--bends"), "pedal": !args.iter().any(|a| a == "--no-pedal")});
    // Isolate first (a stem from the best installed separator; default from the model) at Quality fast|high.
    for (flag, key) in [("--isolate", "isolate"), ("--quality", "quality")] {
        if let Some(v) = arg_value(args, flag) {
            p[key] = json!(v);
        }
    }
    for (flag, key) in [("--onset", "onset_threshold"), ("--frame", "frame_threshold"), ("--min-ms", "min_note_ms")] {
        if let Some(v) = arg_value(args, flag) {
            match v.parse::<f64>() {
                Ok(x) => p[key] = json!(x),
                Err(_) => return fail(format!("{flag} must be a number")),
            }
        }
    }
    let t0 = std::time::Instant::now();
    let r = match e.execute("ml.midi", &p) {
        Ok(r) => r,
        Err(err) => return fail(err),
    };
    let made = r["made"].as_array().and_then(|m| m.first()).and_then(|m| m["track"].as_u64()).unwrap_or(0);
    if let Err(err) = e.execute("file.export_midi", &json!({"path": out, "tracks": [made]})) {
        return fail(err);
    }
    let notes: u64 = r["made"].as_array().map_or(0, |m| m.iter().filter_map(|x| x["notes"].as_u64()).sum());
    let how: Vec<&str> = r["notes"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    outln!("wrote {out}: {notes} notes at {bpm} bpm in {:.2} s ({})", t0.elapsed().as_secs_f64(), how.join("; "));
    ExitCode::SUCCESS
}

/// `ml text-midi`: MIDI-LLM on `--prompt`; every instrument track into one format-1 .mid.
fn ml_text_midi(args: &[String], device: soundcraft_ml::Device, dir: &std::path::Path) -> ExitCode {
    use soundcraft_model::Session;
    let Some(out) = args.get(1).filter(|a| !a.starts_with("--")) else { return fail("ml text-midi OUT.mid --prompt TEXT …") };
    let Some(prompt) = arg_value(args, "--prompt") else { return fail("ml text-midi needs --prompt TEXT (what to compose)") };
    let bpm: f64 = match arg_value(args, "--bpm").map(|s| s.parse::<f64>()) {
        None => 120.0,
        Some(Ok(b)) if b.is_finite() && (20.0..=999.0).contains(&b) => b,
        Some(_) => return fail("--bpm must be a number from 20 to 999"),
    };
    let mut s = Session { sample_rate: soundcraft_time::SampleRate::HZ_48000, ..Session::default() };
    if let Err(e) = s.tempo.set_tempo(0, bpm) {
        return fail(e);
    }
    let mut e = Engine::new(s);
    let mut p = json!({"prompt": prompt, "start": 0, "device": device.id(), "models_dir": dir.to_string_lossy()});
    for (flag, key) in [("--seconds", "seconds"), ("--temperature", "temperature")] {
        if let Some(v) = arg_value(args, flag) {
            match v.parse::<f64>() {
                Ok(x) => p[key] = json!(x),
                Err(_) => return fail(format!("{flag} must be a number")),
            }
        }
    }
    if let Some(v) = arg_value(args, "--seed") {
        p["seed"] = json!(v);
    }
    eprintln!("  composing (MIDI-LLM; up to a few minutes on the CPU)");
    let t0 = std::time::Instant::now();
    let r = match e.execute("ml.text_midi", &p) {
        Ok(r) => r,
        Err(err) => return fail(err),
    };
    let made: Vec<&Value> = r["made"].as_array().map(|m| m.iter().collect()).unwrap_or_default();
    let tracks: Vec<u64> = made.iter().filter_map(|m| m["track"].as_u64()).collect();
    if let Err(err) = e.execute("file.export_midi", &json!({"path": out, "tracks": tracks})) {
        return fail(err);
    }
    let notes: u64 = made.iter().filter_map(|m| m["notes"].as_u64()).sum();
    let names: Vec<&str> = made.iter().filter_map(|m| m["name"].as_str()).collect();
    outln!(
        "wrote {out}: {notes} notes on {} tracks ({}) at {bpm} bpm in {:.1} s, seed {}",
        tracks.len(),
        names.join(", "),
        t0.elapsed().as_secs_f64(),
        r["seed"].as_str().unwrap_or("?")
    );
    ExitCode::SUCCESS
}

/// `ml chords`: BTC on the file; the segments as JSON (`{"segments": [...]}`) or a `.lab` file
/// (`start end label` per line, as mir_eval reads).
fn ml_chords(args: &[String], buf: soundcraft_audio_io::AudioBuffer, out: &str, device: soundcraft_ml::Device, dir: std::path::PathBuf) -> ExitCode {
    use soundcraft_ml::{Cancel, ChordOptions, Ctx, JobKind};
    let bass = if args.iter().any(|a| a == "--bass") {
        true
    } else if args.iter().any(|a| a == "--no-bass") {
        false
    } else {
        buf.channels.len() >= 2
    };
    let job = soundcraft_ml::Job {
        kind: JobKind::Chords(ChordOptions { bass }),
        audio: buf.channels,
        rate: buf.sample_rate,
        device,
        models_dir: dir,
        quality: soundcraft_ml::Quality::Fast,
    };
    let t0 = std::time::Instant::now();
    let r = match soundcraft_ml::pipeline::run(&job, &Ctx::new(Cancel::new(), |_, _| {})) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let segs = r.chords.unwrap_or_default();
    let body = if out.to_ascii_lowercase().ends_with(".lab") {
        segs.iter().map(|s| format!("{:.3} {:.3} {}\n", s.start, s.end, s.label)).collect::<String>()
    } else {
        serde_json::to_string_pretty(&json!({"segments": segs, "bass": bass})).unwrap_or_default()
    };
    if let Err(e) = std::fs::write(out, body) {
        return fail(format!("{out}: {e}"));
    }
    let names: Vec<&str> = segs.iter().filter(|s| s.is_chord()).map(|s| s.name.as_str()).collect();
    outln!("wrote {out}: {} chords in {:.2} s ({}): {}", names.len(), t0.elapsed().as_secs_f64(), r.notes.join("; "), names.join(" "));
    ExitCode::SUCCESS
}

/// `ml lyrics`: the file as a clip at 0, Transcribe Lyrics on it (the same command the app runs),
/// and the track's lyrics exported as `.lrc` or text; `.json` writes the words and lines with
/// their times in seconds.
fn ml_lyrics(args: &[String], buf: soundcraft_audio_io::AudioBuffer, out: &str, dir: &std::path::Path) -> ExitCode {
    use soundcraft_model::{ChannelFormat, Clip, Session, TrackKind};
    let Ok(rate) = soundcraft_time::SampleRate::new(buf.sample_rate) else { return fail(format!("unsupported sample rate {} Hz", buf.sample_rate)) };
    let mut s = Session { sample_rate: rate, ..Session::default() };
    let len = buf.channels.first().map_or(0, Vec::len) as i64;
    if len == 0 {
        return fail("the file holds no audio");
    }
    let format = if buf.channels.len() >= 2 { ChannelFormat::Stereo } else { ChannelFormat::Mono };
    let t = s.add_track(TrackKind::Audio, format, Some("Vocal"));
    let src = soundcraft_engine::io::add_source(&mut s, "Vocal", buf, None, soundcraft_audio_io::FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(id, "Vocal", src, 0, 0, len));
    let mut e = Engine::new(s);
    let mut p = json!({"track": t.0, "models_dir": dir.to_string_lossy(), "language": arg_value(args, "--language").unwrap_or_else(|| "en".into())});
    if let Some(prompt) = arg_value(args, "--prompt") {
        p["prompt"] = json!(prompt);
    }
    if let Some(q) = arg_value(args, "--quality") {
        p["quality"] = json!(q);
    }
    if args.iter().any(|a| a == "--skip-silence") {
        p["skip_silence"] = json!(true);
    }
    let t0 = std::time::Instant::now();
    let r = match e.execute("ml.lyrics", &p) {
        Ok(r) => r,
        Err(err) => return fail(err),
    };
    let secs = t0.elapsed().as_secs_f64();
    let lower = out.to_ascii_lowercase();
    let written = if lower.ends_with(".json") {
        e.execute("lyrics.list", &json!({"track": t.0}))
            .and_then(|v| {
                std::fs::write(out, serde_json::to_string_pretty(&v).unwrap_or_default())
                    .map_err(|err| soundcraft_engine::EngineError::Io(err.to_string()))
            })
            .map(|_| json!({}))
    } else if lower.ends_with(".txt") {
        e.execute("file.export_lyrics_text", &json!({"path": out, "track": t.0}))
    } else {
        e.execute("file.export_lyrics", &json!({"path": out, "track": t.0}))
    };
    let made = &r["made"][0];
    if let Err(err) = written {
        // No words heard is not a failure of the run: say so.
        if made["words"].as_u64() == Some(0) {
            outln!("no lyrics heard ({secs:.1} s)");
            return ExitCode::SUCCESS;
        }
        return fail(err);
    }
    let how: Vec<&str> = r["notes"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    outln!("wrote {out}: {} lines, {} words in {secs:.1} s ({})", made["lines"], made["words"], how.join("; "));
    for line in made["text"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        outln!("  {line}");
    }
    ExitCode::SUCCESS
}

fn info(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else { return fail("info FILE") };
    if soundcraft_model::is_session_path(path) {
        let mut e = Engine::default();
        match soundcraft_engine::io::open_session(&mut e, path) {
            Ok(missing) => warn_missing(&missing),
            Err(err) => return fail(err),
        }
        outln!("{}", soundcraft_engine::inspect::session_text(&e));
        return ExitCode::SUCCESS;
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return fail(format!("{path}: {e}")),
    };
    let ext = std::path::Path::new(path).extension().and_then(|x| x.to_str());
    match soundcraft_audio_io::probe(&bytes, ext) {
        Ok(i) => {
            outln!("{}", serde_json::to_string_pretty(&json!({"format": i.format, "sample_format": i.sample_format, "sample_rate": i.sample_rate, "channels": i.channels, "frames": i.frames, "seconds": i.duration_secs(), "bwf": i.bwf})).unwrap_or_default());
            ExitCode::SUCCESS
        }
        Err(e) => fail(format!("{path}: {e}")),
    }
}

fn convert(args: &[String]) -> ExitCode {
    let (Some(input), Some(output)) = (args.first(), args.get(1)) else { return fail("convert IN OUT") };
    let rate = match parse_rate(arg_value(args, "--rate").as_deref()) {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    let bit_depth = match parse_bit_depth(arg_value(args, "--bit-depth").as_deref()) {
        Ok(b) => b,
        Err(e) => return fail(e),
    };
    let bytes = match std::fs::read(input) {
        Ok(b) => b,
        Err(e) => return fail(format!("{input}: {e}")),
    };
    let ext = std::path::Path::new(input).extension().and_then(|x| x.to_str());
    let (_, mut buf) = match soundcraft_audio_io::decode(&bytes, ext) {
        Ok(x) => x,
        Err(e) => return fail(format!("{input}: {e}")),
    };
    if let Some(rate) = rate {
        buf.channels = soundcraft_dsp::offline::resample(&buf.channels, buf.sample_rate, rate);
        buf.sample_rate = rate;
    }
    if args.iter().any(|a| a == "--normalize") {
        soundcraft_dsp::offline::normalize(&mut buf.channels, -0.1, false);
    }
    let out_ext = std::path::Path::new(output).extension().and_then(|x| x.to_str()).unwrap_or("wav").to_ascii_lowercase();
    let Some(format) = soundcraft_audio_io::encode_format_for(&out_ext) else {
        return fail(format!("{output}: cannot write `.{out_ext}` files (supported: {})", soundcraft_audio_io::ENCODE_EXTENSIONS));
    };
    let opts = soundcraft_audio_io::EncodeOptions { format, bit_depth, dither: true, bwf: None, bitrate_kbps: 0 };
    match soundcraft_audio_io::encode(&buf, &opts).map_err(|e| e.to_string()).and_then(|b| std::fs::write(output, b).map_err(|e| e.to_string())) {
        Ok(()) => {
            outln!("wrote {output} ({} ch, {} Hz, {:.2} s)", buf.num_channels(), buf.sample_rate, buf.duration_secs());
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}

fn parse_rate(v: Option<&str>) -> Result<Option<u32>, String> {
    match v {
        None => Ok(None),
        Some(r) => match r.parse::<u32>() {
            Ok(hz) if hz > 0 => Ok(Some(hz)),
            _ => Err(format!("--rate: expected a sample rate in Hz, got `{r}`")),
        },
    }
}

fn parse_bit_depth(v: Option<&str>) -> Result<soundcraft_audio_io::BitDepth, String> {
    match v {
        None | Some("24") => Ok(soundcraft_audio_io::BitDepth::Int24),
        Some("16") => Ok(soundcraft_audio_io::BitDepth::Int16),
        Some("32") | Some("32f") => Ok(soundcraft_audio_io::BitDepth::Float32),
        Some(b) => Err(format!("--bit-depth: expected 16, 24 or 32f, got `{b}`")),
    }
}

/// One stderr line per media file a session refers to but could not load (the exit code is unchanged).
fn missing_warnings(missing: &[String]) -> Vec<String> {
    missing.iter().map(|m| format!("warning: missing media: {m}")).collect()
}

fn warn_missing(missing: &[String]) {
    for line in missing_warnings(missing) {
        eprintln!("{line}");
    }
}

fn load_engine(args: &[String]) -> Result<Engine, String> {
    if let Some(p) = arg_value(args, "--in") {
        let mut e = Engine::default();
        let missing = soundcraft_engine::io::open_session(&mut e, &p).map_err(|e| e.to_string())?;
        warn_missing(&missing);
        Ok(e)
    } else if args.iter().any(|a| a == "--demo" || a == "--sample") {
        Ok(soundcraft_engine::demo::demo_engine())
    } else {
        Ok(Engine::default())
    }
}

fn finish(e: &mut Engine, args: &[String]) -> ExitCode {
    if let Some(out) = arg_value(args, "--bounce") {
        let mut p = json!({"path": out});
        if let Some(s) = arg_value(args, "--start") {
            p["start"] = time_arg(&s);
        }
        if let Some(s) = arg_value(args, "--end") {
            p["end"] = time_arg(&s);
        }
        match e.execute("file.bounce_mix", &p) {
            Ok(v) => outln!("{v}"),
            Err(err) => return fail(err),
        }
    }
    if let Some(path) = arg_value(args, "--save") {
        match e.execute("session.save", &json!({"path": path, "replace": true})) {
            Ok(v) => outln!("{v}"),
            Err(err) => return fail(err),
        }
    }
    if args.iter().any(|a| a == "--inspect") {
        outln!("{}", serde_json::to_string_pretty(&soundcraft_engine::inspect::session(e, false)).unwrap_or_default());
    }
    ExitCode::SUCCESS
}

/// `12.5` → seconds; anything else is passed through as a time string.
fn time_arg(s: &str) -> Value {
    s.parse::<f64>().map_or_else(|_| json!(s), |f| json!({"seconds": f}))
}

/// `ID` or `ID=JSON` → command id and params; malformed JSON is an error, not `{}`.
fn parse_cmd_spec(spec: &str) -> Result<(String, Value), String> {
    match spec.split_once('=') {
        Some((id, p)) => serde_json::from_str::<Value>(p).map(|v| (id.to_string(), v)).map_err(|err| format!("{id}: bad JSON: {err}")),
        None => Ok((spec.to_string(), json!({}))),
    }
}

fn run(args: &[String]) -> ExitCode {
    let mut e = match load_engine(args) {
        Ok(e) => e,
        Err(err) => return fail(err),
    };
    let mut i = 0;
    while i < args.len() {
        if args.get(i).is_some_and(|a| a == "--cmd")
            && let Some(spec) = args.get(i + 1)
        {
            let (id, params) = match parse_cmd_spec(spec) {
                Ok(v) => v,
                Err(err) => return fail(err),
            };
            match e.execute(&id, &params) {
                Ok(v) => outln!("{id}: {v}"),
                Err(err) => return fail(format!("{id}: {err}")),
            }
            i += 1;
        }
        i += 1;
    }
    finish(&mut e, args)
}

fn script(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else { return fail("script FILE") };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => return fail(format!("{path}: {e}")),
    };
    let mut e = match load_engine(args) {
        Ok(e) => e,
        Err(err) => return fail(err),
    };
    let keep = args.iter().any(|a| a == "--keep-going");
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (id, p) = line.split_once(char::is_whitespace).unwrap_or((line, "{}"));
        let params = match serde_json::from_str::<Value>(p.trim()) {
            Ok(v) => v,
            Err(err) => return fail(format!("{path}:{}: bad JSON: {err}", n + 1)),
        };
        match e.execute(id, &params) {
            Ok(v) => outln!("{id}: {v}"),
            Err(err) => {
                eprintln!("{path}:{}: {id}: {err}", n + 1);
                if !keep {
                    return ExitCode::FAILURE;
                }
            }
        }
    }
    finish(&mut e, args)
}

fn commands(args: &[String]) -> ExitCode {
    let filter = args.first().cloned().unwrap_or_default();
    let e = Engine::default();
    for c in soundcraft_engine::command_specs() {
        if !filter.is_empty() && !c.id.contains(&filter) && !c.label.to_ascii_lowercase().contains(&filter.to_ascii_lowercase()) {
            continue;
        }
        let i = c.info(&e);
        outln!("{:<36} {:<34} {}", i.id, i.label, i.menu.join(" > "));
    }
    ExitCode::SUCCESS
}

fn describe(args: &[String]) -> ExitCode {
    let Some(id) = args.first() else { return fail("describe ID") };
    match soundcraft_engine::find_command(id) {
        Some(c) => {
            outln!("{}", serde_json::to_string_pretty(&c.info(&Engine::default())).unwrap_or_default());
            ExitCode::SUCCESS
        }
        None => fail(format!("unknown command `{id}`")),
    }
}

/// Control-channel methods. Every other dotted id is an engine command.
fn is_control_method(method: &str) -> bool {
    matches!(
        method,
        "engine.execute"
            | "command"
            | "engine.commands"
            | "engine.parity"
            | "session.inspect"
            | "document.inspect"
            | "ui.inspect"
            | "ui.menu.list"
            | "ui.menu.invoke"
            | "ui.set"
            | "ui.move"
            | "ui.click"
            | "ui.drag"
            | "ui.key"
            | "ui.text"
            | "ui.screenshot"
            | "app.quit"
    )
}

/// A command id is shorthand for `engine.execute`. Documented control methods stay methods.
fn app_request(method: &str, params: Value) -> (String, Value) {
    if is_control_method(method) || !method.contains('.') {
        (method.to_string(), params)
    } else {
        ("engine.execute".to_string(), json!({"command": method, "params": params}))
    }
}

fn app(args: &[String]) -> ExitCode {
    let port = arg_value(args, "--port").or_else(|| soundcraft_engine::brand::env_var("CONTROL_PORT")).unwrap_or_else(|| "7979".into());
    let positional: Vec<&String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| !(a.starts_with("--") || (*i > 0 && args.get(i - 1).is_some_and(|p| p == "--port"))))
        .map(|(_, a)| a)
        .collect();
    let Some(method) = positional.first() else { return fail("app [--port P] METHOD [JSON]") };
    let params = match positional.get(1) {
        Some(p) => match serde_json::from_str::<Value>(p) {
            Ok(v) => v,
            Err(err) => return fail(format!("{method}: bad JSON: {err}")),
        },
        None => json!({}),
    };
    let mut r = Remote::new(&port);
    // A bare command id is shorthand for engine.execute. Control-channel methods stay methods.
    let (m, p) = app_request(method, params);
    match r.call(&m, p) {
        Ok(v) => {
            outln!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            ExitCode::SUCCESS
        }
        Err(e) => fail(e),
    }
}

/// What `mcp` was asked for.
struct McpArgs {
    connect: Option<String>,
    demo: bool,
    roots: Option<std::sync::Arc<AutomationRoots>>,
}

/// Parse the `mcp` options up front: a malformed or misspelled root flag must not leave the
/// server running unconfined.
fn parse_mcp_args(args: &[String]) -> Result<McpArgs, String> {
    let mut connect = None;
    let mut demo = false;
    let mut roots_args = automation_args::AutomationArgs::new(None, None);
    let mut it = args.iter().cloned();
    while let Some(a) = it.next() {
        if roots_args.consume(&a, &mut it)? {
            continue;
        }
        match a.as_str() {
            "--connect" => connect = Some(it.next().ok_or("--connect needs a port or host:port")?),
            "--demo" => demo = true,
            other if other.starts_with("--automation-") => return Err(format!("unknown mcp option `{other}`")),
            _ => {}
        }
    }
    let roots = AutomationRoots::new(roots_args.read_root.as_deref(), roots_args.write_root.as_deref())?;
    if roots.is_some() && connect.is_some() {
        return Err("--automation-read-root and --automation-write-root confine the in-process server; a running app is not confined by them, \
so drop --connect (or use --connect without the roots)"
            .into());
    }
    Ok(McpArgs { connect, demo, roots })
}

fn mcp(args: &[String]) -> ExitCode {
    let McpArgs { connect, demo, roots } = match parse_mcp_args(args) {
        Ok(a) => a,
        Err(err) => return fail(err),
    };
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let r = if let Some(port) = connect {
        let mut s = Server::new(Remote::new(&port));
        s.serve(stdin.lock(), stdout.lock())
    } else {
        // The headless engine hosts plugins; the bridged mode (`--connect`) has no engine and
        // leaves the settings folder alone.
        use_plugin_scan_cache();
        // `--demo` is the operator's own choice, made before the roots come into force.
        let e = if demo { soundcraft_engine::demo::demo_engine() } else { Engine::default() };
        let mut h = Headless::new(e);
        if let Some(roots) = roots {
            // Every thread of this process, not only the server's: nothing reaches past the roots.
            file_access::confine_process(roots.clone());
            let show = |root: Option<&std::path::Path>| root.map_or_else(|| "none".to_string(), |r| r.display().to_string());
            eprintln!("freedawm-cli: files confined: read root {}, write root {}", show(roots.read_root()), show(roots.write_root()));
            h = h.with_automation_roots(Some(roots));
        }
        let mut s = Server::new(h);
        s.serve(stdin.lock(), stdout.lock())
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(e),
    }
}

fn parity(args: &[String]) -> ExitCode {
    let p = soundcraft_engine::catalog::parity_json();
    let mut md = String::new();
    md.push_str("# Parity report\n\nGenerated by `freedawm-cli parity --write docs/parity-checklist.md` (also `cargo xtask parity`).\n");
    md.push_str("It compares the incumbent's menu tree (feature names observed black-box, `crates/engine/catalog/menus.txt`) with FreeDAWm's command registry.\n");
    md.push_str("UI-layer commands (windows, Process) are counted by the app's own report (`engine.parity` over the control channel).\n\n");
    md.push_str(&format!(
        "**Menu items implemented by the engine: {} / {} ({}%)**\n\n| Menu | Implemented | Total |\n|---|---:|---:|\n",
        p["implemented"], p["total"], p["percent"]
    ));
    for m in p["per_menu"].as_array().cloned().unwrap_or_default() {
        md.push_str(&format!("| {} | {} | {} |\n", m["menu"].as_str().unwrap_or(""), m["implemented"], m["total"]));
    }
    md.push_str("\n## Not yet implemented\n\n");
    for m in p["missing"].as_array().cloned().unwrap_or_default() {
        md.push_str(&format!("- {}\n", m.as_str().unwrap_or("")));
    }
    if let Some(path) = arg_value(args, "--write") {
        if let Err(e) = std::fs::write(&path, &md) {
            return fail(format!("{path}: {e}"));
        }
        outln!("wrote {path}: {}% ({}/{})", p["percent"], p["implemented"], p["total"]);
    } else {
        outln!("{md}");
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_ids_are_execute_shorthand_and_control_methods_stay_methods() {
        let (m, p) = app_request("engine.history", json!({}));
        assert_eq!(m, "engine.execute");
        assert_eq!(p["command"], "engine.history");
        let (m, p) = app_request("ui.theme", json!({"mode": "light"}));
        assert_eq!(m, "engine.execute");
        assert_eq!(p["params"]["mode"], "light");
        let (m, _) = app_request("session.new", json!({"name": "Session A"}));
        assert_eq!(m, "engine.execute");
        let (m, _) = app_request("track.new", json!({}));
        assert_eq!(m, "engine.execute");
        for method in ["session.inspect", "engine.commands", "engine.parity", "ui.screenshot", "app.quit"] {
            assert_eq!(app_request(method, json!({})).0, method);
        }
    }

    #[test]
    fn convert_rejects_invalid_rate_and_bit_depth() {
        assert_eq!(parse_rate(None), Ok(None));
        assert_eq!(parse_rate(Some("44100")), Ok(Some(44_100)));
        assert!(parse_rate(Some("4410x")).is_err());
        assert!(parse_rate(Some("0")).is_err());
        assert!(matches!(parse_bit_depth(None), Ok(soundcraft_audio_io::BitDepth::Int24)));
        assert!(matches!(parse_bit_depth(Some("16")), Ok(soundcraft_audio_io::BitDepth::Int16)));
        assert!(matches!(parse_bit_depth(Some("32f")), Ok(soundcraft_audio_io::BitDepth::Float32)));
        assert!(parse_bit_depth(Some("8")).is_err());
    }

    #[test]
    fn missing_media_gets_one_warning_each() {
        assert!(missing_warnings(&[]).is_empty());
        let w = missing_warnings(&["a.wav".to_string(), "b.wav: decode failed".to_string()]);
        assert_eq!(w, vec!["warning: missing media: a.wav", "warning: missing media: b.wav: decode failed"]);
    }

    #[test]
    fn cmd_spec_parses_id_and_json() {
        assert_eq!(parse_cmd_spec("a.b").unwrap(), ("a.b".to_string(), json!({})));
        assert_eq!(parse_cmd_spec("a.b={\"x\":1}").unwrap(), ("a.b".to_string(), json!({"x": 1})));
    }

    #[test]
    fn cmd_spec_rejects_truncated_json() {
        let err = parse_cmd_spec("a.b={\"x\":").unwrap_err();
        assert!(err.starts_with("a.b: bad JSON"), "{err}");
    }

    #[test]
    fn help_text_is_only_the_usage_block() {
        let help = help_text();
        assert!(help.starts_with("freedawm-cli info FILE"));
        assert!(help.ends_with("freedawm-cli ml bench IN.wav [--seconds 30]   HTDemucs speed on the GPU (DirectML) and the CPU"));
        assert!(help.contains("freedawm-cli plugins                          list built-in plugins"));
        assert!(help.contains("--automation-read-root DIR") && help.contains("--automation-write-root DIR"));
        assert!(!help.contains("```"));
        assert!(!help.contains("#![") && !help.contains("use serde_json"));
    }
}
