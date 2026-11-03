//! Embeds the Library: every file under `library/{Players,Targets,Presets/Bands,Songs/Factory}`
//! plus `library/factory.json`, as `include_bytes!` entries sorted by path, written to
//! `OUT_DIR/embedded_files.rs` (included by `src/library/embedded.rs`).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const DIRS: &[&str] = &["Players", "Targets", "Presets/Bands", "Songs/Factory"];

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_default());
    let lib = manifest.join("library");
    println!("cargo::rerun-if-changed={}", lib.display());
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for d in DIRS {
        collect(&lib, &lib.join(d), &mut files);
    }
    let factory = lib.join("factory.json");
    if factory.is_file() {
        files.push(("factory.json".to_string(), factory));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = String::from("&[\n");
    for (rel, path) in &files {
        let _ = writeln!(out, "    ({rel:?}, include_bytes!({:?}) as &[u8]),", path.to_string_lossy());
    }
    out.push_str("]\n");
    let dest = PathBuf::from(std::env::var_os("OUT_DIR").unwrap_or_default()).join("embedded_files.rs");
    if let Err(e) = std::fs::write(&dest, out) {
        println!("cargo::warning={}: {e}", dest.display());
    }
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        println!("cargo::warning={}: unreadable", dir.display());
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(root, &p, out);
        } else if p.extension().is_some_and(|x| x == "json")
            && let Ok(rel) = p.strip_prefix(root)
        {
            out.push((rel.to_string_lossy().replace('\\', "/"), p));
        }
    }
}
