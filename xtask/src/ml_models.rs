//! `cargo xtask ml-models [--scripts DIR] [--venv DIR] [--models DIR] [--force] [ids…]`: exports
//! the local-ML models (PyTorch → ONNX, parity checks, golden files) with the export scripts'
//! `export_all.py` and installs them into the model folder the app reads.
//!
//! - `--scripts`: the `tools/ml-export` folder (default: this checkout's, when it has one; the
//!   scripts live on the `feat/ml-export` branch until it is merged).
//! - `--venv`: the Python venv (default: `$ML_VENV`, else the first `ml-venv` folder next to this
//!   checkout or one of its parents). Create it as the scripts' README says.
//! - `--models`: where to install (default: `FREEDAWM_MODELS_DIR` or `SOUNDCRAFT_MODELS_DIR`, else
//!   `%LOCALAPPDATA%\FreeDAWm\models`); passed to the scripts as `ML_MODELS_DIR`.
//! - `--force` and model ids go to `export_all.py` (it skips up-to-date models).
//! - `--copy-from DIR`: instead of exporting, install models another export already made (each
//!   `DIR/<id>/` with a `card.json`): copied into the model folder, every file checked against its
//!   card's sha256 (a model already there and matching is left alone).

use std::path::{Path, PathBuf};
use std::process::Command;

fn flag(args: &[String], name: &str) -> Option<PathBuf> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(PathBuf::from)
}

pub(crate) fn default_models() -> PathBuf {
    if let Some(d) = std::env::var_os("FREEDAWM_MODELS_DIR").or_else(|| std::env::var_os("SOUNDCRAFT_MODELS_DIR")).filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
        .unwrap_or_else(std::env::temp_dir);
    // As the app (`soundcraft_ml::models`): the pre-rename folder until the new one exists.
    let (new, legacy) = (base.join("FreeDAWm").join("models"), base.join("SoundCraft").join("models"));
    if !new.exists() && legacy.is_dir() { legacy } else { new }
}

fn find_venv(root: &Path) -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("ML_VENV").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(v));
    }
    root.ancestors().map(|a| a.join("ml-venv")).find(|v| v.is_dir())
}

fn python(venv: &Path) -> PathBuf {
    let win = venv.join("Scripts").join("python.exe");
    if win.is_file() { win } else { venv.join("bin").join("python") }
}

/// The sha256 of a file, as lowercase hex.
pub(crate) fn sha256(path: &Path) -> Result<String, String> {
    use sha2::Digest;
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(buf.get(..n).unwrap_or_default());
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The model files a card names, with their sha256: (relative path, sha256).
pub(crate) fn card_files(card: &Path) -> Result<Vec<(String, String)>, String> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(card).map_err(|e| format!("{}: {e}", card.display()))?)
        .map_err(|e| format!("{}: {e}", card.display()))?;
    let one = |f: &serde_json::Value| (f["file"].as_str().unwrap_or("").to_string(), f["sha256"].as_str().unwrap_or("").to_string());
    let mut files = vec![one(&v)];
    files.extend(v["extra_files"].as_array().map(|a| a.iter().map(one).collect::<Vec<_>>()).unwrap_or_default());
    if files.iter().any(|(f, s)| f.is_empty() || f.contains("..") || s.is_empty()) {
        return Err(format!("{}: a file without a name or sha256", card.display()));
    }
    Ok(files)
}

/// The licence note on a card (`licence` or `license`), first sentence: printed with each
/// install so the person sees what they are installing under (the app shows the same status).
fn card_licence(card: &Path) -> String {
    let v: serde_json::Value = std::fs::read_to_string(card).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let note = v.get("licence").or_else(|| v.get("license")).and_then(serde_json::Value::as_str).unwrap_or("").trim();
    if note.is_empty() { "not stated on the card".to_string() } else { note.split(". ").next().unwrap_or(note).to_string() }
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if src.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| format!("{} -> {}: {e}", src.display(), dst.display()))?;
        }
    }
    Ok(())
}

/// `--copy-from`: install every model of `from` into `models`, checked.
fn copy_models(from: &Path, models: &Path) -> Result<(), String> {
    let mut n = 0;
    for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let dir = entry.map_err(|e| e.to_string())?.path();
        let card = dir.join("card.json");
        if !card.is_file() {
            continue;
        }
        let id = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let files = card_files(&card)?;
        let dst = models.join(&id);
        let current = dst.join("card.json").is_file()
            && card_files(&dst.join("card.json")).is_ok_and(|f| f == files)
            && files.iter().all(|(f, s)| sha256(&dst.join(f)).is_ok_and(|h| h == *s));
        if current {
            println!("ml-models: {id} up to date in {}", dst.display());
        } else {
            let tmp = models.join(format!(".{id}.partial"));
            let _ = std::fs::remove_dir_all(&tmp);
            copy_tree(&dir, &tmp)?;
            for (f, s) in &files {
                let got = sha256(&tmp.join(f))?;
                if got != *s {
                    let _ = std::fs::remove_dir_all(&tmp);
                    return Err(format!("{id}: {f} has sha256 {got}, its card says {s}"));
                }
            }
            let _ = std::fs::remove_dir_all(&dst);
            std::fs::rename(&tmp, &dst).map_err(|e| format!("{} -> {}: {e}", tmp.display(), dst.display()))?;
            println!("ml-models: installed {id} into {} (sha256 checked); licence: {}", dst.display(), card_licence(&card));
        }
        n += 1;
    }
    if n == 0 {
        return Err(format!("{}: no model folders (each needs a card.json)", from.display()));
    }
    Ok(())
}

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    if let Some(from) = flag(args, "--copy-from") {
        let models = flag(args, "--models").unwrap_or_else(default_models);
        return copy_models(&from, &models);
    }
    let scripts = flag(args, "--scripts").unwrap_or_else(|| root.join("tools").join("ml-export"));
    let entry = scripts.join("export_all.py");
    if !entry.is_file() {
        return Err(format!(
            "{} not found: pass --scripts <tools/ml-export folder> (the export scripts are on the feat/ml-export branch)",
            entry.display()
        ));
    }
    let venv =
        flag(args, "--venv").or_else(|| find_venv(root)).ok_or("no Python venv: pass --venv DIR or set ML_VENV (see tools/ml-export/README.md)")?;
    let py = python(&venv);
    if !py.is_file() {
        return Err(format!("{}: no Python in this venv", py.display()));
    }
    let models = flag(args, "--models").unwrap_or_else(default_models);
    let mut pass: Vec<&str> = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if matches!(a.as_str(), "--scripts" | "--venv" | "--models") {
            skip = true;
            continue;
        }
        pass.push(a);
    }
    println!("ml-models: {} {} {}  →  {}", py.display(), entry.display(), pass.join(" "), models.display());
    let st = Command::new(&py)
        .arg(&entry)
        .args(&pass)
        .env("ML_MODELS_DIR", &models)
        .current_dir(&scripts)
        .status()
        .map_err(|e| format!("{}: {e}", py.display()))?;
    if st.success() { Ok(()) } else { Err(format!("export_all.py failed ({st})")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_scripts_say_where_to_find_them() {
        let err = run(Path::new("C:/nowhere"), &["--scripts".into(), "C:/nowhere/tools".into()]).unwrap_err();
        assert!(err.contains("--scripts") && err.contains("feat/ml-export"), "{err}");
    }
}
