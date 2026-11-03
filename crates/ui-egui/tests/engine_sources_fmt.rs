//! `cargo fmt` does not reach the FireStation engines' UI files: `build.rs` declares them in a
//! file it writes, pulled in with `include!`, which rustfmt does not expand. This runs rustfmt on
//! them instead. To format them: `rustfmt --edition 2024 crates/ui-egui/src/engines/*.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn engine_ui_sources_are_rustfmt_clean() {
    let dir = Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_else(|| env!("CARGO_MANIFEST_DIR").into())).join("src/engines");
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "rs")).collect();
    files.sort();
    assert!(!files.is_empty());
    let rustfmt = std::env::var_os("RUSTFMT").unwrap_or_else(|| "rustfmt".into());
    let out = match Command::new(rustfmt).args(["--edition", "2024", "--check"]).args(&files).output() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("SKIPPED: rustfmt could not run: {e}");
            return;
        }
    };
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() && out.stdout.is_empty() && stderr.contains("not installed") {
        eprintln!("SKIPPED: {stderr}");
        return;
    }
    assert!(
        out.status.success(),
        "the engine UI files are not formatted: run `rustfmt --edition 2024 crates/ui-egui/src/engines/*.rs`\n{}{stderr}",
        String::from_utf8_lossy(&out.stdout)
    );
}
