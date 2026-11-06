//! Fidelity audio for the FireStation engine ports (local only, git-ignored):
//!
//! - `cargo xtask fidelity-inputs` cuts the shared input excerpts from the owner's `Test Samples`
//!   and writes a synthetic sine/sweep/impulse file, all 48 kHz stereo 24-bit.
//! - `cargo xtask fidelity-refs <engine>` renders every case of `<engine>/cases.json` through the
//!   unmodified C++ `<X>_Render.exe`, writing `refs/<case>.wav` (24-bit, Ruling R1).
//!
//! Ruling R2: `renderfile.cpp`'s `--sr` is a plain sample-rate tag (it only reaches
//! `prepareToPlay` and the output header; the samples are never resampled). So every case reads
//! the same 48 kHz input file whatever its rate, and the Rust test feeds those identical samples
//! to `prepare(sr)` the same way.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};

/// Engine short name → (directory under the FireStation root, render tool name without
/// `_Render`), from each engine's own registration (`soundcraft_dsp::plugins::fs::EngineReg`).
pub fn engines() -> Vec<(&'static str, &'static str, &'static str)> {
    soundcraft_dsp::plugins::fs::engines().iter().filter_map(|e| e.fidelity).map(|f| (f.name, f.source_dir, f.render_tool)).collect()
}

/// Input excerpts: (file written, `Test Samples` source without extension).
const EXCERPTS: &[(&str, &str)] =
    &[("guitar", "acoustic_guitar"), ("drums", "drums"), ("rhodes", "rhodes"), ("song", "song_intro"), ("piano", "wonky_piano")];

const RATE: u32 = 48_000;
const START_S: f64 = 1.0;
const LEN_S: f64 = 5.0;

pub fn firestation_root() -> PathBuf {
    std::env::var_os("FIRESTATION_ROOT").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\projects\A18\FireStation"))
}

fn fidelity_dir(repo: &Path) -> PathBuf {
    repo.join("crates/dsp/tests/fidelity")
}

fn write_wav(path: &Path, channels: Vec<Vec<f32>>) -> Result<(), String> {
    let buf = AudioBuffer { sample_rate: RATE, channels };
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int24, dither: false, bwf: None, bitrate_kbps: 0 };
    let bytes = soundcraft_audio_io::encode(&buf, &opts).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// `cargo xtask fidelity-inputs`.
pub fn inputs(repo: &Path) -> Result<(), String> {
    let samples = firestation_root().join("Test Samples");
    let out_dir = fidelity_dir(repo).join("inputs");
    let len = (LEN_S * f64::from(RATE)) as usize;
    for (name, src) in EXCERPTS {
        let path = samples.join(format!("{src}.wav"));
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let (info, buf) = soundcraft_audio_io::decode(&bytes, Some("wav")).map_err(|e| format!("{}: {e}", path.display()))?;
        let secs = buf.frames() as f64 / f64::from(info.sample_rate.max(1));
        if secs < START_S + LEN_S {
            println!(
                "WARNING: {} is {secs:.2} s long, shorter than the {:.1} s an excerpt needs; {name}.wav ends in silence",
                path.display(),
                START_S + LEN_S
            );
        }
        // Cut first (with a little margin for the resampler's window), then convert to 48 kHz once:
        // from here on both the C++ tool and the Rust test read exactly these samples.
        let from = (START_S * f64::from(info.sample_rate)) as usize;
        let src_len = (LEN_S * f64::from(info.sample_rate)).ceil() as usize + 64;
        let mut chans: Vec<Vec<f32>> = buf.channels.iter().map(|c| c.iter().copied().skip(from).take(src_len).collect::<Vec<f32>>()).collect();
        if chans.len() == 1 {
            chans.push(chans[0].clone());
        }
        chans.truncate(2);
        if chans.len() != 2 {
            return Err(format!("{}: no audio channels", path.display()));
        }
        let mut chans = soundcraft_dsp::offline::resample(&chans, info.sample_rate, RATE);
        for c in &mut chans {
            c.resize(len, 0.0);
        }
        let dst = out_dir.join(format!("{name}.wav"));
        write_wav(&dst, chans)?;
        println!("{} <- {} ({} Hz, {} ch)", dst.display(), path.display(), info.sample_rate, info.channels);
    }

    // Synthetic: 1 s sine at -6 dBFS, 2 s log sweep 20 Hz..20 kHz at -12 dBFS, 0.5 s of 1 Hz impulses.
    let sr = f64::from(RATE);
    let mut mono = Vec::with_capacity((3.5 * sr) as usize);
    let sine_amp = 10f64.powf(-6.0 / 20.0);
    for n in 0..RATE as usize {
        mono.push((sine_amp * (std::f64::consts::TAU * 1000.0 * n as f64 / sr).sin()) as f32);
    }
    let sweep_amp = 10f64.powf(-12.0 / 20.0);
    let (f0, f1, dur) = (20.0f64, 20_000.0f64, 2.0f64);
    let k = (f1 / f0).ln();
    for n in 0..(dur * sr) as usize {
        let t = n as f64 / sr;
        let phase = std::f64::consts::TAU * f0 * dur / k * ((t * k / dur).exp() - 1.0);
        mono.push((sweep_amp * phase.sin()) as f32);
    }
    for n in 0..(0.5 * sr) as usize {
        mono.push(if n % RATE as usize == 0 { 1.0 } else { 0.0 });
    }
    let dst = out_dir.join("synthetic.wav");
    write_wav(&dst, vec![mono.clone(), mono])?;
    println!("{}", dst.display());
    Ok(())
}

/// A case value as the render tool's `--set` takes it (the APVTS value).
fn value_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(if *b { "1" } else { "0" }.to_string()),
        _ => None,
    }
}

/// `cargo xtask fidelity-refs <engine>`.
pub fn refs(repo: &Path, args: &[String]) -> Result<(), String> {
    let engines = engines();
    let names: Vec<&str> = engines.iter().map(|e| e.0).collect();
    let engine = args.first().ok_or_else(|| format!("usage: cargo xtask fidelity-refs <{}>", names.join("|")))?;
    let &(_, dir, tool) = engines.iter().find(|e| e.0 == engine).ok_or_else(|| format!("unknown engine {engine}; one of {}", names.join(", ")))?;
    let root = firestation_root();
    let engine_dir = root.join(dir);
    let exe = engine_dir.join(format!("build/{tool}_Render_artefacts/Release/{tool}_Render.exe"));
    let exe_time = std::fs::metadata(&exe).and_then(|m| m.modified()).map_err(|e| format!("{}: {e}", exe.display()))?;

    // Stale build: the references must come from the sources as they are now.
    let source = engine_dir.join("Source");
    let mut stale = Vec::new();
    collect_newer(&source, exe_time, &mut stale)?;
    if !stale.is_empty() {
        return Err(format!(
            "{} is older than its sources (stale build; rebuild it first):\n  {}",
            exe.display(),
            stale.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n  ")
        ));
    }
    println!("{} is current with {}", exe.display(), source.display());

    let fid = fidelity_dir(repo);
    let cases_path = fid.join(engine).join("cases.json");
    let text = std::fs::read_to_string(&cases_path).map_err(|e| format!("{}: {e}", cases_path.display()))?;
    let cases: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", cases_path.display()))?;
    let cases = cases.as_array().ok_or_else(|| format!("{}: expected an array", cases_path.display()))?;
    let refs_dir = fid.join(engine).join("refs");
    std::fs::create_dir_all(&refs_dir).map_err(|e| format!("{}: {e}", refs_dir.display()))?;

    for case in cases {
        let name = case["name"].as_str().ok_or("a case without a name")?;
        let input = case["input"].as_str().ok_or_else(|| format!("{name}: no input"))?;
        let sr = case["sr"].as_u64().ok_or_else(|| format!("{name}: no sr"))?;
        let block = case["block"].as_u64().ok_or_else(|| format!("{name}: no block"))?;
        let in_path = fid.join("inputs").join(input);
        if !in_path.exists() {
            return Err(format!("{}: missing; run `cargo xtask fidelity-inputs` first", in_path.display()));
        }
        let out_path = refs_dir.join(format!("{name}.wav"));
        // The stored hash ties the reference to this exact case entry; drop the old one first so a
        // failed render never leaves a stale pair that still looks current.
        let hash_path = refs_dir.join(format!("{name}.hash"));
        let _ = std::fs::remove_file(&hash_path);
        let mut cmd = Command::new(&exe);
        cmd.arg(&in_path).arg(&out_path).args(["--sr", &sr.to_string(), "--block", &block.to_string()]);
        if let Some(values) = case["values"].as_object() {
            for (id, v) in values {
                let v = value_text(v).ok_or_else(|| format!("{name}: {id} is not a number"))?;
                cmd.arg("--set").arg(format!("{id}={v}"));
            }
        }
        // `after`: set once the first values have settled (the tool pumps the message loop) and
        // the editor is open, the way a user turns a knob after picking a circuit. DistEngine's
        // knob memory recalls a circuit's own Drive/Tone on a circuit change, so only values set
        // after it reach the engine as written.
        if let Some(after) = case.get("after").and_then(|a| a.as_object()) {
            cmd.arg("--editor");
            for (id, v) in after {
                let v = value_text(v).ok_or_else(|| format!("{name}: after {id} is not a number"))?;
                cmd.arg("--after").arg(format!("{id}={v}"));
            }
        }
        // `props`: properties on the plugin's saved state (the tool's `--prop`). AmpEngine's IRs:
        // `irPath<Amp>` names a bundled IR (`assets/amp-irs/<name>`, a copy of the owner's
        // `IRs/<name>`). Loading one makes the plugin remember it in the owner's settings
        // (`%APPDATA%\FireStation\AmpEngine\AmpEngine.settings`), so only the IR already
        // remembered there for that amp is accepted, passed exactly as written there: nothing of
        // the owner's changes, and the plugin's constructor has that IR in place before the
        // render (no load race, no crossfade from another response).
        if let Some(props) = case.get("props").and_then(|a| a.as_object()) {
            for (key, v) in props {
                let v = v.as_str().ok_or_else(|| format!("{name}: prop {key} is not a string"))?;
                let value = if key.starts_with("irPath") { remembered_ir(&engine_dir, key, v)? } else { v.to_string() };
                cmd.arg("--prop").arg(format!("{key}={value}"));
            }
        }
        let out = cmd.output().map_err(|e| format!("{}: {e}", exe.display()))?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() || stdout.contains("NO SUCH PARAMETER") || stdout.contains("NON-FINITE") || !out_path.exists() {
            return Err(format!("{name}: render failed ({})\n{stdout}{}", out.status, String::from_utf8_lossy(&out.stderr)));
        }
        let mtime = exe_time.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let stamp = format!(
            "case {:016x}
exe-mtime {mtime}
",
            fnv1a64(case_key(case).as_bytes())
        );
        std::fs::write(&hash_path, stamp).map_err(|e| format!("{}: {e}", hash_path.display()))?;
        println!("{name}: {}", out_path.display());
    }
    println!("{} references in {}", cases.len(), refs_dir.display());
    Ok(())
}

/// The path the owner's AmpEngine settings remember for `key`, if it is the bundled IR `name`
/// (`<engine dir>/IRs/<name>`); an error otherwise (rendering with another IR would overwrite it).
fn remembered_ir(engine_dir: &Path, key: &str, name: &str) -> Result<String, String> {
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from).ok_or("APPDATA is not set")?;
    let settings = appdata.join("FireStation").join("AmpEngine").join("AmpEngine.settings");
    let text = std::fs::read_to_string(&settings).map_err(|e| format!("{}: {e}", settings.display()))?;
    let needle = format!("name=\"{key}\" val=\"");
    let at = text.find(&needle).ok_or_else(|| format!("{}: no {key} remembered", settings.display()))?;
    let rest = &text[at + needle.len()..];
    let remembered = rest.split('"').next().unwrap_or("").replace("&amp;", "&").replace("&apos;", "'");
    let want = engine_dir.join("IRs").join(name);
    let norm = |p: &str| p.replace('/', "\\").to_ascii_lowercase();
    if norm(&remembered) != norm(&want.display().to_string()) {
        return Err(format!(
            "{key}: the owner's settings remember {remembered:?}, not {}; a render with another IR would overwrite the owner's choice",
            want.display()
        ));
    }
    Ok(remembered)
}

pub(crate) fn collect_newer(dir: &Path, than: SystemTime, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_newer(&path, than, out)?;
        } else if entry.metadata().and_then(|m| m.modified()).map_err(|e| format!("{}: {e}", path.display()))? > than {
            out.push(path);
        }
    }
    Ok(())
}

/// Canonical text of one case entry: name, input, sr, block, then the values sorted by id.
/// MUST stay identical to `case_key` in `crates/dsp/tests/fidelity.rs` (both pin the same vector).
pub fn case_key(case: &serde_json::Value) -> String {
    let mut s = format!(
        "name={}
input={}
sr={}
block={}
",
        case["name"].as_str().unwrap_or(""),
        case["input"].as_str().unwrap_or(""),
        case["sr"].as_u64().unwrap_or(0),
        case["block"].as_u64().unwrap_or(0)
    );
    let mut values: Vec<(&String, &serde_json::Value)> = case["values"].as_object().map(|o| o.iter().collect()).unwrap_or_default();
    values.sort_by(|a, b| a.0.cmp(b.0));
    for (k, v) in values {
        let v = match v {
            serde_json::Value::Number(n) => n.as_f64().map(|x| format!("{x:?}")).unwrap_or_default(),
            serde_json::Value::Bool(b) => b.to_string(),
            _ => "?".to_string(),
        };
        s.push_str(&format!(
            "{k}={v}
"
        ));
    }
    // Only cases that have `after` values carry this section, so every other key is unchanged.
    if let Some(after) = case.get("after").and_then(|a| a.as_object()) {
        let mut after: Vec<(&String, &serde_json::Value)> = after.iter().collect();
        after.sort_by(|a, b| a.0.cmp(b.0));
        s.push_str(
            "after
",
        );
        for (k, v) in after {
            let v = v.as_f64().map(|x| format!("{x:?}")).unwrap_or_else(|| "?".to_string());
            s.push_str(&format!(
                "{k}={v}
"
            ));
        }
    }
    // The same for `props` (state properties: AmpEngine's IRs).
    if let Some(props) = case.get("props").and_then(|a| a.as_object()) {
        let mut props: Vec<(&String, &serde_json::Value)> = props.iter().collect();
        props.sort_by(|a, b| a.0.cmp(b.0));
        s.push_str(
            "props
",
        );
        for (k, v) in props {
            s.push_str(&format!(
                "{k}={}
",
                v.as_str().unwrap_or("?")
            ));
        }
    }
    s
}

/// FNV-1a, 64-bit: stable across toolchains (unlike `DefaultHasher`).
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_hash_is_pinned() {
        // The same vector is pinned in crates/dsp/tests/fidelity.rs, so the two sides agree.
        let case: serde_json::Value =
            serde_json::from_str(r#"{"name":"x","input":"song.wav","sr":48000,"block":512,"values":{"mix":0.5,"circuit":2}}"#).unwrap();
        assert_eq!(
            case_key(&case),
            "name=x
input=song.wav
sr=48000
block=512
circuit=2.0
mix=0.5
"
        );
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(format!("{:016x}", fnv1a64(case_key(&case).as_bytes())), PINNED);
    }

    const PINNED: &str = "ba4cd375db776a93";

    #[test]
    fn after_values_extend_the_key() {
        let case: serde_json::Value =
            serde_json::from_str(r#"{"name":"x","input":"song.wav","sr":48000,"block":512,"values":{"circuit":2},"after":{"tone":0.25,"drive":1}}"#)
                .unwrap();
        assert!(
            case_key(&case).ends_with(
                "circuit=2.0
after
drive=1.0
tone=0.25
"
            ),
            "{}",
            case_key(&case)
        );
        assert_eq!(format!("{:016x}", fnv1a64(case_key(&case).as_bytes())), PINNED_AFTER);
    }

    const PINNED_AFTER: &str = "4723c66fb471e523";
}
