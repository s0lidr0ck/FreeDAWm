//! `cargo xtask build-asio`: check the ASIO build prerequisites, then build the
//! app (`freedawm`) with `--features asio`, and the plugin scanner helper (`freedawm-scan`) that must sit
//! next to it. It only reads the environment and the file system;
//! it never installs anything or writes any config.

use std::path::{Path, PathBuf};
use std::process::Command;

const LLVM_MSG: &str = concat!(
    r"LLVM not found: run `winget install LLVM.LLVM`, then set LIBCLANG_PATH = 'C:\Program Files\LLVM\bin' under [env] in %USERPROFILE%\.cargo\config.toml",
    "\n  (the folder must contain libclang.dll; the config lines are:\n   [env]\n   ",
    r#"LIBCLANG_PATH = "C:\\Program Files\\LLVM\\bin""#,
    ")"
);
const SDK_MSG: &str = concat!(
    r"ASIO SDK not found: download Steinberg's ASIO SDK, unzip it to C:\projects\A18\FireStation\DAW\.asio-sdk, and set CPAL_ASIO_DIR to that folder under [env] in %USERPROFILE%\.cargo\config.toml",
    "\n  (the folder must contain common/asio.h; the config lines are:\n   [env]\n   ",
    r#"CPAL_ASIO_DIR = "C:\\projects\\A18\\FireStation\\DAW\\.asio-sdk""#,
    ")"
);

/// One message per missing prerequisite (empty when ready). Both variables are read
/// from the process environment, which cargo fills from the user-level config.
pub(crate) fn prerequisites(env: &dyn Fn(&str) -> Option<String>, exists: &dyn Fn(&Path) -> bool) -> Vec<String> {
    prerequisites_for(cfg!(windows), env, exists)
}

fn prerequisites_for(windows: bool, env: &dyn Fn(&str) -> Option<String>, exists: &dyn Fn(&Path) -> bool) -> Vec<String> {
    if !windows {
        return vec!["ASIO builds are Windows-only".to_string()];
    }
    let has = |var: &str, rel: &str| env(var).filter(|v| !v.trim().is_empty()).is_some_and(|v| exists(&Path::new(v.trim()).join(rel)));
    let mut missing = Vec::new();
    if !has("LIBCLANG_PATH", "libclang.dll") {
        missing.push(LLVM_MSG.to_string());
    }
    if !has("CPAL_ASIO_DIR", "common/asio.h") {
        missing.push(SDK_MSG.to_string());
    }
    missing
}

pub fn run(_args: &[String]) -> Result<(), String> {
    let missing = prerequisites(&|k| std::env::var(k).ok(), &|p| p.exists());
    if !missing.is_empty() {
        for m in &missing {
            eprintln!("- {m}");
        }
        return Err(format!("{} ASIO prerequisite(s) missing; nothing was built", missing.len()));
    }
    let root = PathBuf::from(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_else(|| env!("CARGO_MANIFEST_DIR").into())).join("..");
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "--release", "-p", "soundcraft", "--features", "asio"])
        .current_dir(&root)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("cargo build --release -p soundcraft --features asio failed".into());
    }
    // The app scans plugins only through this helper (in its own processes), found next to it:
    // cargo puts both in the same `release` folder.
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "--release", "-p", "soundcraft-scan"])
        .current_dir(&root)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("cargo build --release -p soundcraft-scan failed".into());
    }
    let target = std::env::var("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|_| root.join("target"));
    let release = target.join("release");
    let exe = |name: &str| release.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    for (name, what) in [("freedawm", "the app"), ("freedawm-scan", "the plugin scanner")] {
        if !exe(name).is_file() {
            return Err(format!("{what} was not built at {}", exe(name).display()));
        }
    }
    println!("ASIO build ready: {} (plugin scanner: {})", exe("freedawm").display(), exe("freedawm-scan").display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_env(k: &str) -> Option<String> {
        match k {
            "LIBCLANG_PATH" => Some("fake-llvm-bin".into()),
            "CPAL_ASIO_DIR" => Some("fake-sdk".into()),
            _ => None,
        }
    }
    fn all_exist(p: &Path) -> bool {
        p.ends_with("libclang.dll") || p.ends_with("common/asio.h")
    }

    #[test]
    fn ready_when_both_are_present() {
        assert!(prerequisites_for(true, &fake_env, &all_exist).is_empty());
    }

    #[test]
    fn each_missing_prerequisite_is_named() {
        let none = |_: &str| None;
        let both = prerequisites_for(true, &none, &all_exist);
        assert_eq!(both.len(), 2);
        assert!(both[0].contains("winget install LLVM.LLVM") && both[0].contains("LIBCLANG_PATH"));
        assert!(both[1].contains(".asio-sdk") && both[1].contains("CPAL_ASIO_DIR"));
        // Variables set but the files are not there.
        let nothing = |_: &Path| false;
        assert_eq!(prerequisites_for(true, &fake_env, &nothing).len(), 2);
        // Only the SDK missing.
        let no_sdk = |p: &Path| p.ends_with("libclang.dll");
        let m = prerequisites_for(true, &fake_env, &no_sdk);
        assert_eq!(m.len(), 1);
        assert!(m[0].contains("CPAL_ASIO_DIR"));
        // Blank variables count as unset.
        let blank = |_: &str| Some("  ".to_string());
        assert_eq!(prerequisites_for(true, &blank, &all_exist).len(), 2);
    }

    #[test]
    fn non_windows_is_refused() {
        assert_eq!(prerequisites_for(false, &fake_env, &all_exist), vec!["ASIO builds are Windows-only".to_string()]);
    }
}
