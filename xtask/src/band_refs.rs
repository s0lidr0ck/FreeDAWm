//! Band reference events (local only, git-ignored), from MIDI Studio's own C++:
//!
//! `cargo xtask band-refs [--library <dir>] [name…]` builds `tools/band-ref` (MIDI Studio's
//! `MIDIStudio` target compiled into `target/band-ref`; the MS tree is only read), copies the
//! Library (`crates/band/library`, or `--library`) to `%TEMP%\band-ref-lib`, and plays every
//! scenario of `crates/band/tests/scenarios/*.json` (or the named ones) through an offline
//! `MidiStudioProcessor`, writing `crates/band/tests/refs/<name>.events`. The scenario format and
//! the stepping rules are in `tools/band-ref/README.md`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::fidelity::{collect_newer, firestation_root, fnv1a64};

/// The CMake that ships with Visual Studio 18 (MIDI Studio's `Tools\configure.bat` uses the same VS).
const CMAKE: &str = "C:/Program Files/Microsoft Visual Studio/18/Community/Common7/IDE/CommonExtensions/Microsoft/CMake/CMake/bin/cmake.exe";
const GENERATOR: &str = "Visual Studio 18 2026";

/// The `# case` text of a scenario: FNV-1a 64 of its file bytes, 16 lowercase hex digits. `band_ref.exe`
/// writes the same on a ref's first line, so a ref made from another version of the file is stale.
pub fn scenario_hash(bytes: &[u8]) -> String {
    format!("{:016x}", fnv1a64(bytes))
}

/// What a ref's `# case` line hashes: the scenario file's bytes, then, when it names a `library_overlay`
/// (`<scenarios>/../fixtures/libraries/<name>`), for each `Players/*.json` / `Targets/*.json` there (sorted by
/// `<Players|Targets>/<file name>`, byte order) a newline, that path, a newline and the file's bytes. An edited overlay
/// file so makes its scenarios' refs stale. band_ref's `caseBytes` and the Rust runner's `common::case_bytes` agree.
pub fn case_bytes(scenario: &Path, bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    let text = String::from_utf8_lossy(bytes);
    let overlay = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
        .ok()
        .and_then(|j| j.get("library_overlay").and_then(|v| v.as_str()).map(str::to_string));
    let (Some(name), Some(dir)) = (overlay, scenario.parent().and_then(Path::parent)) else { return out };
    let dir = dir.join("fixtures").join("libraries").join(name);
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for sub in ["Players", "Targets"] {
        let Ok(rd) = std::fs::read_dir(dir.join(sub)) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let json = p.extension().is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case("json"));
            if let (true, true, Some(n)) = (json, p.is_file(), e.file_name().to_str()) {
                files.push((format!("{sub}/{n}"), p));
            }
        }
    }
    files.sort();
    for (rel, p) in files {
        out.push(b'\n');
        out.extend_from_slice(rel.as_bytes());
        out.push(b'\n');
        out.extend_from_slice(&std::fs::read(&p).unwrap_or_default());
    }
    out
}

/// Where the reference event files live (git-ignored).
pub fn refs_dir(repo: &Path) -> PathBuf {
    repo.join("crates/band/tests/refs")
}

pub fn scenarios_dir(repo: &Path) -> PathBuf {
    repo.join("crates/band/tests/scenarios")
}

/// MIDI Studio's checkout: `$MIDISTUDIO_DIR`, else `<FireStation>/MIDI Studio`.
fn midistudio_dir() -> PathBuf {
    std::env::var_os("MIDISTUDIO_DIR").map(PathBuf::from).unwrap_or_else(|| firestation_root().join("MIDI Studio"))
}

fn run(cmd: &mut Command, what: &str) -> Result<(), String> {
    let st = cmd.status().map_err(|e| format!("{what}: {e}"))?;
    if st.success() { Ok(()) } else { Err(format!("{what} failed ({st})")) }
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", from.display()))?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| format!("{} -> {}: {e}", src.display(), dst.display()))?;
        }
    }
    Ok(())
}

/// Configures (when the cache is missing or names another MIDI Studio) and builds `BandRef`, Release.
fn build(repo: &Path, ms: &Path) -> Result<PathBuf, String> {
    let build_dir = repo.join("target/band-ref");
    let ms_cmake = ms.display().to_string().replace('\\', "/");
    let cache = std::fs::read_to_string(build_dir.join("CMakeCache.txt")).unwrap_or_default();
    if !cache.lines().any(|l| l == format!("MIDISTUDIO_DIR:PATH={ms_cmake}")) {
        run(
            Command::new(CMAKE)
                .arg("-S")
                .arg(repo.join("tools/band-ref"))
                .arg("-B")
                .arg(&build_dir)
                .args(["-G", GENERATOR, "-A", "x64"])
                .arg(format!("-DMIDISTUDIO_DIR={ms_cmake}")),
            "cmake configure",
        )?;
    }
    let build = || {
        run(
            Command::new(CMAKE).arg("--build").arg(&build_dir).args(["--config", "Release", "--target", "BandRef", "--", "/m", "/v:m"]),
            "cmake --build (BandRef)",
        )
    };
    build()?;
    let exe = build_dir.join("BandRef_artefacts/Release/BandRef.exe");
    // Stale build (as fidelity-refs): the refs must come from MS's sources as they are now. A newer file the
    // build does not compile leaves the exe untouched, so relink once before refusing.
    for attempt in 0..2 {
        let exe_time = std::fs::metadata(&exe).and_then(|m| m.modified()).map_err(|e| format!("{}: {e}", exe.display()))?;
        let mut stale = Vec::new();
        collect_newer(&ms.join("Source"), exe_time, &mut stale)?;
        if stale.is_empty() {
            println!("{} is current with {}", exe.display(), ms.join("Source").display());
            return Ok(exe);
        }
        if attempt == 1 {
            return Err(format!(
                "{} is older than MIDI Studio's sources (stale build):\n  {}",
                exe.display(),
                stale.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\n  ")
            ));
        }
        std::fs::remove_file(&exe).map_err(|e| format!("{}: {e}", exe.display()))?;
        build()?;
    }
    Err("unreachable stale-build loop".into())
}

/// `cargo xtask band-refs [--library <dir>] [name…]`.
pub fn refs(repo: &Path, args: &[String]) -> Result<(), String> {
    let mut library = repo.join("crates/band/library");
    let mut names = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--library" {
            library = PathBuf::from(it.next().ok_or("--library needs a folder")?);
        } else {
            names.push(a.trim_end_matches(".json").to_string());
        }
    }
    if !library.join("Players").is_dir() {
        return Err(format!("{}: not a band Library (no Players folder)", library.display()));
    }

    let dir = scenarios_dir(repo);
    let mut scenarios: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    scenarios.sort();
    let stem = |p: &Path| p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    if !names.is_empty() {
        for n in &names {
            if !scenarios.iter().any(|p| stem(p) == *n) {
                return Err(format!("no scenario {n} in {}", dir.display()));
            }
        }
        scenarios.retain(|p| names.contains(&stem(p)));
    }

    let ms = midistudio_dir();
    let exe = build(repo, &ms)?;

    let lib_copy = std::env::temp_dir().join("band-ref-lib");
    if lib_copy.exists() {
        std::fs::remove_dir_all(&lib_copy).map_err(|e| format!("{}: {e}", lib_copy.display()))?;
    }
    copy_dir(&library, &lib_copy)?;

    let out_dir = refs_dir(repo);
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    for path in &scenarios {
        let name = stem(path);
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let out = out_dir.join(format!("{name}.events"));
        let tmp = out_dir.join(format!("{name}.events.tmp"));
        // A failed run never leaves an older ref that still looks current.
        let _ = std::fs::remove_file(&out);
        let _ = std::fs::remove_file(&tmp);
        run(Command::new(&exe).arg("--scenario").arg(path).arg("--out").arg(&tmp).arg("--library").arg(&lib_copy), &format!("band_ref {name}"))?;
        let text = std::fs::read_to_string(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        let want = format!("# case {}", scenario_hash(&case_bytes(path, &bytes)));
        if text.lines().next() != Some(want.as_str()) {
            return Err(format!("{name}: the ref's first line is not `{want}`"));
        }
        std::fs::rename(&tmp, &out).map_err(|e| format!("{} -> {}: {e}", tmp.display(), out.display()))?;
        println!("{name}: {} events -> {}", text.lines().count().saturating_sub(1), out.display());
    }
    let _ = std::fs::remove_dir_all(&lib_copy);
    println!("{} references in {}", scenarios.len(), out_dir.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_hash_is_pinned() {
        // band_ref.exe prints the same text on its `# case` line; the Rust runner compares the two.
        assert_eq!(scenario_hash(b"{}"), PINNED);
        assert_eq!(scenario_hash(b"").len(), 16);
    }

    const PINNED: &str = "08f44b07b5901a25";

    #[test]
    fn an_overlay_folds_its_files_into_the_case() {
        let root = std::env::temp_dir().join(format!("xtask-case-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (scen, lib) = (root.join("scenarios"), root.join("fixtures/libraries/o"));
        assert!(std::fs::create_dir_all(&scen).is_ok() && std::fs::create_dir_all(lib.join("Targets")).is_ok());
        assert!(std::fs::create_dir_all(lib.join("Players")).is_ok());
        let path = scen.join("x.json");
        let bytes = br#"{"name": "x", "library_overlay": "o"}"#;
        assert!(std::fs::write(lib.join("Targets/T.json"), b"{1}").is_ok() && std::fs::write(lib.join("Players/P.json"), b"{2}").is_ok());
        assert!(std::fs::write(lib.join("Targets/notes.txt"), b"ignored").is_ok());
        let mut want = bytes.to_vec();
        want.extend_from_slice(b"\nPlayers/P.json\n{2}\nTargets/T.json\n{1}");
        assert_eq!(case_bytes(&path, bytes), want);
        assert_eq!(case_bytes(&path, b"{\"name\": \"y\"}"), b"{\"name\": \"y\"}".to_vec());
        let before = scenario_hash(&case_bytes(&path, bytes));
        assert!(std::fs::write(lib.join("Targets/T.json"), b"{3}").is_ok());
        assert_ne!(scenario_hash(&case_bytes(&path, bytes)), before);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refs_live_beside_the_scenarios() {
        assert_eq!(refs_dir(Path::new("repo")), Path::new("repo").join("crates/band/tests/refs"));
    }
}
