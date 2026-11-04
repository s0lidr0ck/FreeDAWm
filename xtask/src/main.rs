//! `cargo xtask <ci|layers|assets|parity|wasm>` — repository gates; `fidelity-inputs` and
//! `fidelity-refs <engine>` make the local FireStation fidelity audio; `band-refs [name…]` makes the
//! band's C++ reference events; `fs-tables <engine>` converts an engine's C++ table headers to Rust;
//! `ml-models` exports and installs the local-ML models (see `ml_models.rs`); `browser-thumbs
//! [id…]` renders the plugin browser's built-in card pictures (see `browser_thumbs.rs`).

mod asio;
mod band_refs;
mod browser_thumbs;
mod fidelity;
mod fs_tables;
mod ico;
mod ml_models;
mod version;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn root() -> PathBuf {
    Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_else(|| env!("CARGO_MANIFEST_DIR").into()))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn cargo(args: &[&str]) -> Result<(), String> {
    run_cargo(Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())).args(args), args)
}

fn run_cargo(cmd: &mut Command, args: &[&str]) -> Result<(), String> {
    let st = cmd.current_dir(root()).status().map_err(|e| e.to_string())?;
    if st.success() { Ok(()) } else { Err(format!("cargo {} failed", args.join(" "))) }
}

/// Layer table: crate name (minus `soundcraft-`) → layer. Apps and xtask are exempt.
const LAYERS: &[(&str, u8)] = &[
    ("time", 0),
    ("audio-io", 0),
    ("midi", 0),
    ("video", 0),
    ("plugin-scan", 0),
    ("sysmem", 0),
    ("dsp", 1),
    ("ml", 1),
    ("band", 1),
    ("vocal", 1),
    ("clap-host", 1),
    ("vst3-host", 1),
    ("au-host", 1),
    ("model", 2),
    ("mix", 3),
    ("engine", 4),
    ("playback", 4),
    ("asio-latency", 4),
    ("automation", 5),
    ("net", 5),
    ("ui-egui", 6),
];
/// Standalone format crates: no workspace dependencies at all (publishable on their own).
const STANDALONE: &[&str] = &["time", "audio-io", "midi", "video", "dsp"];
const UI_CRATES: &[&str] = &["craft-ui", "egui", "eframe", "winit", "egui_kittest", "rfd", "egui-wgpu", "egui_glow"];
const EXEMPT: &[&str] = &["soundcraft", "soundcraft-cli", "soundcraft-scan", "soundcraft-web", "xtask"];

fn layers() -> Result<(), String> {
    let out =
        Command::new("cargo").args(["metadata", "--format-version", "1", "--no-deps"]).current_dir(root()).output().map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let mut problems = Vec::new();
    for p in v["packages"].as_array().cloned().unwrap_or_default() {
        let name = p["name"].as_str().unwrap_or("").to_string();
        if EXEMPT.contains(&name.as_str()) {
            continue;
        }
        let short = name.trim_start_matches("soundcraft-");
        let Some(&(_, layer)) = LAYERS.iter().find(|(n, _)| *n == short) else {
            problems.push(format!("{name}: not registered in xtask LAYERS"));
            continue;
        };
        for d in p["dependencies"].as_array().cloned().unwrap_or_default() {
            let dn = d["name"].as_str().unwrap_or("");
            let kind = d["kind"].as_str().unwrap_or("normal");
            if UI_CRATES.contains(&dn) && layer < 6 && kind != "dev" {
                problems.push(format!("{name} (L{layer}) depends on UI crate {dn}"));
            }
            if let Some(ds) = dn.strip_prefix("soundcraft-") {
                if STANDALONE.contains(&short) && kind != "dev" {
                    problems.push(format!("{name} is standalone but depends on {dn}"));
                }
                if let Some(&(_, dl)) = LAYERS.iter().find(|(n, _)| *n == ds)
                    && dl > layer
                    && kind != "dev"
                {
                    problems.push(format!("{name} (L{layer}) depends upward on {dn} (L{dl})"));
                }
            }
        }
    }
    if problems.is_empty() {
        println!("layers: ok");
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

const ASSET_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "svg", "ico", "icns", "ttf", "otf", "ttc", "woff", "woff2", "wav", "aif", "aiff", "flac", "mp3", "ogg", "gif", "webp",
    "lex",
];

fn assets() -> Result<(), String> {
    let out = Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .current_dir(root())
        .output()
        .map_err(|e| e.to_string())?;
    let files = String::from_utf8_lossy(&out.stdout).to_string();
    let attribution = std::fs::read_to_string(root().join("ATTRIBUTION.md")).map_err(|e| format!("ATTRIBUTION.md: {e}"))?;
    let mut missing = Vec::new();
    for f in files.lines() {
        let ext = Path::new(f).extension().and_then(|x| x.to_str()).unwrap_or("").to_ascii_lowercase();
        if !ASSET_EXT.contains(&ext.as_str()) {
            continue;
        }
        // Generated icon trees are covered by their directory row.
        let covered = attribution.contains(&format!("`{f}`"))
            || Path::new(f).ancestors().skip(1).any(|a| !a.as_os_str().is_empty() && attribution.contains(&format!("`{}/`", a.display())));
        if !covered {
            missing.push(f.to_string());
        }
    }
    if missing.is_empty() {
        println!("assets: ok");
        Ok(())
    } else {
        Err(format!("assets without an ATTRIBUTION.md row:\n  {}", missing.join("\n  ")))
    }
}

fn parity() -> Result<(), String> {
    cargo(&["run", "-q", "-p", "soundcraft-cli", "--", "parity", "--write", "docs/parity-checklist.md"])
}

/// Crates that must compile for the web.
const WASM: &[&str] = &[
    "soundcraft-time",
    "soundcraft-audio-io",
    "soundcraft-midi",
    "soundcraft-video",
    "soundcraft-dsp",
    "soundcraft-band",
    "soundcraft-vocal",
    "soundcraft-model",
    "soundcraft-mix",
    "soundcraft-engine",
    "soundcraft-playback",
    "soundcraft-automation",
    "soundcraft-ui-egui",
];

fn wasm() -> Result<(), String> {
    let mut args = vec!["check", "--target", "wasm32-unknown-unknown"];
    for c in WASM {
        args.push("-p");
        args.push(c);
    }
    if root().join("apps/soundcraft-web/Cargo.toml").exists() {
        args.push("-p");
        args.push("soundcraft-web");
    }
    // No rustc wrapper: web-sys's rustc command line (~47k characters of `--cfg feature=...`)
    // passes Windows's 32k limit. Cargo hands it over as an `@argfile`, but sccache (0.18) expands
    // the file and spawns rustc with the full line, which fails with os error 206 ("The filename
    // or extension is too long"). An empty RUSTC_WRAPPER overrides `build.rustc-wrapper`.
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.args(&args).env("RUSTC_WRAPPER", "").env_remove("CARGO_BUILD_RUSTC_WRAPPER");
    run_cargo(&mut cmd, &args)
}

/// `cargo test --workspace`, plus the test-harness arguments in `XTASK_TEST_ARGS` (split on
/// whitespace) if set: CI uses it to skip timing tests the hosted runners can't keep.
fn test_workspace() -> Result<(), String> {
    let extra = std::env::var("XTASK_TEST_ARGS").unwrap_or_default();
    let mut args = vec!["test", "--workspace"];
    if !extra.trim().is_empty() {
        args.push("--");
        args.extend(extra.split_whitespace());
    }
    cargo(&args)
}

fn ci() -> Result<(), String> {
    let steps: Vec<(&str, Box<dyn Fn() -> Result<(), String>>)> = vec![
        ("fmt", Box::new(|| cargo(&["fmt", "--all", "--", "--check"]))),
        ("clippy", Box::new(|| cargo(&["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"]))),
        ("test", Box::new(test_workspace)),
        ("assets", Box::new(assets)),
        ("layers", Box::new(layers)),
        ("wasm", Box::new(wasm)),
    ];
    let mut summary = Vec::new();
    for (name, f) in &steps {
        match f() {
            Ok(()) => summary.push(format!("ok    {name}")),
            Err(e) => {
                summary.push(format!("FAIL  {name}"));
                println!("{}", summary.join("\n"));
                return Err(e);
            }
        }
    }
    println!("{}", summary.join("\n"));
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("ci") => ci(),
        Some("layers") => layers(),
        Some("assets") => assets(),
        Some("parity") => parity(),
        Some("wasm") => wasm(),
        Some("build-asio") => asio::run(args.get(1..).unwrap_or(&[])),
        Some("version") => version::run(args.get(1..).unwrap_or(&[])),
        Some("ico") => ico::run(args.get(1..).unwrap_or(&[])),
        Some("fidelity-inputs") => fidelity::inputs(&root()),
        Some("fidelity-refs") => fidelity::refs(&root(), args.get(1..).unwrap_or(&[])),
        Some("browser-thumbs") => browser_thumbs::run(&root(), args.get(1..).unwrap_or(&[])),
        Some("band-refs") => band_refs::refs(&root(), args.get(1..).unwrap_or(&[])),
        Some("fs-tables") => fs_tables::run(&root(), args.get(1..).unwrap_or(&[])),
        Some("ml-models") => ml_models::run(&root(), args.get(1..).unwrap_or(&[])),
        _ => {
            Err("usage: cargo xtask <ci|layers|assets|parity|wasm|build-asio|version|fidelity-inputs|fidelity-refs <engine>|band-refs [--library <dir>] [name…]|browser-thumbs [id…]|fs-tables <engine>|ml-models [--scripts DIR] [--venv DIR] [--models DIR] [--force] [ids…] | ml-models --copy-from DIR [--models DIR]>"
                .into())
        }
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}
