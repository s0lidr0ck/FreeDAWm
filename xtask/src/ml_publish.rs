//! `cargo xtask ml-publish [--models DIR] [--out DIR] [--base-url URL] [ids…]`: packs the
//! installed models for the FreeDAWm models release (spec 2026-11-04-model-downloads).
//!
//! For every model of the app's catalogue (`crates/ml/src/catalog.rs`, included below so the
//! manifest is written with the app's own types) it zips the installed folder as
//! `cargo xtask ml-models` installs it (`card.json`, the ONNX and every other file; the `golden/`
//! test data is left out), each file checked against the card's sha256 first, and writes into
//! `--out`:
//! - `<id>.zip` (stored entries, at the zip's root);
//! - `models.json`: per model `id`, `version` (the card's `exported_utc`), `url`, `size`,
//!   `sha256`, `license` (short status), `license_note`, `source`, `personal_use_only`; a model
//!   that is not installed, or whose folder is over GitHub's 2 GiB asset limit, gets
//!   `manual: true` and a `manual_note` instead;
//! - `MODELS-NOTICE.md`: every model, its source and licence.
//!
//! Uploading is a manual `gh release` step (`docs/release-playbook.md`).
//!
//! Given model ids, `models.json` and the notice are the release's current manifest with those
//! models' entries replaced or added (downloaded with `gh`, or `--merge FILE`), so uploading them
//! keeps the other models; `--no-merge` writes only the given models.
//!
//! - `--models`: the installed models (default: as `ml-models`).
//! - `--out`: default `target/models-publish`.
//! - `--base-url`: where the zips will be served (default the `models-v1` release's download URL).

#[path = "../../crates/ml/src/catalog.rs"]
#[allow(dead_code)]
mod catalog;

use catalog::{ASSET_LIMIT, KNOWN_MODELS, MANIFEST_FORMAT, MODELS_REPO, MODELS_TAG, Manifest, ManifestModel};
use std::io::Write;
use std::path::{Path, PathBuf};

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// The release's download URL, ending in `/`.
fn default_base() -> String {
    format!("https://github.com/{MODELS_REPO}/releases/download/{MODELS_TAG}/")
}

/// The files to pack: everything in the folder (following a linked folder) but `golden/` and
/// hidden files, as (path in the zip, file).
fn pack_list(dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
        let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.filter_map(Result::ok).collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || (rel.is_empty() && name == "golden") {
                continue;
            }
            let path = e.path();
            let inner = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            if path.is_dir() {
                walk(&path, &inner, out)?;
            } else if path.is_file() {
                out.push((inner, path));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, "", &mut out)?;
    Ok(out)
}

fn write_zip(files: &[(String, PathBuf)], to: &Path) -> Result<(), String> {
    let tmp = to.with_extension("zip.part");
    let f = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let mut w = zip::ZipWriter::new(std::io::BufWriter::new(f));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
    for (name, path) in files {
        w.start_file(name.as_str(), opts).map_err(|e| format!("{name}: {e}"))?;
        let mut src = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        std::io::copy(&mut src, &mut w).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let mut inner = w.finish().map_err(|e| format!("{}: {e}", tmp.display()))?;
    inner.flush().map_err(|e| format!("{}: {e}", tmp.display()))?;
    drop(inner);
    std::fs::rename(&tmp, to).map_err(|e| format!("{}: {e}", to.display()))
}

fn text<'a>(card: &'a serde_json::Value, keys: &[&str]) -> &'a str {
    keys.iter().find_map(|k| card.get(*k).and_then(serde_json::Value::as_str)).unwrap_or("").trim()
}

/// Puts `entry` in place of the manifest's entry with the same id, or at the end.
fn put(manifest: &mut Manifest, entry: ManifestModel) {
    match manifest.models.iter_mut().find(|e| e.id == entry.id) {
        Some(slot) => *slot = entry,
        None => manifest.models.push(entry),
    }
}

/// Packs the models of `models` into `out`; `limit` is the largest asset (GitHub's 2 GiB).
/// With `live` (the release's current `models.json`), the written manifest and notice are `live`
/// with the packed models' entries replaced or added, so uploading them keeps every other model;
/// a requested model that is not installed keeps its `live` entry.
pub(crate) fn publish(models: &Path, out: &Path, base: &str, only: &[String], limit: u64, live: Option<&Manifest>) -> Result<Manifest, String> {
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let base = if base.ends_with('/') { base.to_string() } else { format!("{base}/") };
    let mut manifest = match live {
        Some(l) => Manifest { format: MANIFEST_FORMAT, release: MODELS_TAG.into(), models: l.models.clone() },
        None => Manifest { format: MANIFEST_FORMAT, release: MODELS_TAG.into(), models: Vec::new() },
    };
    // Song engines are published by the engines workflow (engines.json), not here.
    for km in KNOWN_MODELS.iter().filter(|m| !m.builtin && !catalog::is_engine(m.id) && (only.is_empty() || only.iter().any(|o| o == m.id))) {
        let dir = models.join(km.id);
        let card_path = dir.join("card.json");
        let manual = |note: String| ManifestModel {
            id: km.id.into(),
            name: km.name.into(),
            license: km.license.into(),
            personal_use_only: catalog::is_personal_use(km.license),
            manual: true,
            manual_note: note,
            ..Default::default()
        };
        if !card_path.is_file() {
            if live.is_some_and(|l| l.models.iter().any(|e| e.id == km.id)) {
                println!("ml-publish: {} not installed in {}: its release entry is kept", km.id, models.display());
                continue;
            }
            println!("ml-publish: {} not installed in {}: listed as install manually", km.id, models.display());
            put(
                &mut manifest,
                manual("Not in the models release: install it with `cargo xtask ml-models` (needs the source and a Python venv).".into()),
            );
            continue;
        }
        let card: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&card_path).map_err(|e| format!("{}: {e}", card_path.display()))?)
                .map_err(|e| format!("{}: {e}", card_path.display()))?;
        let note = text(&card, &["license", "licence"]).to_string();
        let status = catalog::licence_status(&note).unwrap_or_else(|| km.license.to_string());
        let source = text(&card, &["source", "source_url", "url"]).to_string();
        let version = text(&card, &["version", "exported_utc"]).to_string();
        let files = pack_list(&dir)?;
        let total: u64 = files.iter().filter_map(|(_, p)| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
        let mut entry = ManifestModel {
            id: km.id.into(),
            name: km.name.into(),
            version,
            license: status.clone(),
            license_note: note,
            source,
            personal_use_only: catalog::is_personal_use(&status),
            ..Default::default()
        };
        if total >= limit {
            println!(
                "ml-publish: {} is {:.2} GB, over the {:.2} GB asset limit: listed as install manually",
                km.id,
                total as f64 / 1e9,
                limit as f64 / 1e9
            );
            entry.manual = true;
            entry.manual_note = format!(
                "Too large for the models release ({:.1} GB; GitHub assets stop at 2 GB). Install it with `cargo xtask ml-models`, or put the author's files and a card.json in the model folder's `{}` folder (see Setup › ML Models).",
                total as f64 / 1e9,
                km.id
            );
            put(&mut manifest, entry);
            continue;
        }
        for (f, want) in crate::ml_models::card_files(&card_path)? {
            let got = crate::ml_models::sha256(&dir.join(&f))?;
            if got != want {
                return Err(format!("{}: {f} has sha256 {got}, its card says {want}: reinstall it before publishing", km.id));
            }
        }
        let zip_path = out.join(format!("{}.zip", km.id));
        println!("ml-publish: packing {} ({} files, {:.0} MB)…", km.id, files.len(), total as f64 / 1e6);
        write_zip(&files, &zip_path)?;
        entry.size = std::fs::metadata(&zip_path).map_err(|e| format!("{}: {e}", zip_path.display()))?.len();
        entry.sha256 = crate::ml_models::sha256(&zip_path)?;
        entry.url = format!("{base}{}.zip", km.id);
        put(&mut manifest, entry);
    }
    let json = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(out.join("models.json"), json + "\n").map_err(|e| format!("models.json: {e}"))?;
    std::fs::write(out.join("MODELS-NOTICE.md"), notice(&manifest)).map_err(|e| format!("MODELS-NOTICE.md: {e}"))?;
    Ok(manifest)
}

fn notice(m: &Manifest) -> String {
    let mut s = String::from(
        "# FreeDAWm models: sources and licences\n\nThe files in this release are the authors' published models converted to ONNX for FreeDAWm \
         (each zip holds the model folder with its `card.json`, which records the source and licence note). The models are the \
         authors' work, under the authors' terms: FreeDAWm's own licence does not cover them.\n\n**Personal use only** marks a model \
         whose author states no licence, or allows non-commercial use only: use it for personal projects; ask the author before anything else. \
         FreeDAWm asks before downloading one.\n\n",
    );
    for e in &m.models {
        s.push_str(&format!("## {} (`{}`)\n\n", e.name, e.id));
        s.push_str(&format!("- Licence: {}{}\n", e.license, if e.personal_use_only { " — **personal use only**" } else { "" }));
        if !e.license_note.is_empty() {
            s.push_str(&format!("- Licence note: {}\n", e.license_note));
        }
        if !e.source.is_empty() {
            s.push_str(&format!("- Source: {}\n", e.source));
        }
        if e.manual {
            s.push_str(&format!("- Not in this release: {}\n", e.manual_note));
        } else {
            s.push_str(&format!("- File: `{}.zip`, {} bytes, sha256 `{}`\n", e.id, e.size, e.sha256));
        }
        s.push('\n');
    }
    s
}

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let models = flag(args, "--models").map_or_else(crate::ml_models::default_models, PathBuf::from);
    let out = flag(args, "--out").map_or_else(|| root.join("target").join("models-publish"), PathBuf::from);
    let base = flag(args, "--base-url").unwrap_or_else(default_base);
    let mut only = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
        } else if matches!(a.as_str(), "--models" | "--out" | "--base-url" | "--merge") {
            skip = true;
        } else if a != "--no-merge" {
            only.push(a.clone());
        }
    }
    // Packing some models: merge into the release's manifest, or uploading ours would drop the rest.
    let live_path = match flag(args, "--merge") {
        Some(p) => Some(PathBuf::from(p)),
        None if !only.is_empty() && !args.iter().any(|a| a == "--no-merge") => Some(download_live(&out)?),
        None => None,
    };
    let live = match &live_path {
        Some(p) => Some(Manifest::parse(&std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?)?),
        None => None,
    };
    let m = publish(&models, &out, &base, &only, ASSET_LIMIT, live.as_ref())?;
    if let Some(p) = &live_path {
        println!("ml-publish: merged into the release's manifest ({})", p.display());
    }
    let zips: Vec<String> = m
        .models
        .iter()
        .filter(|e| !e.manual && (only.is_empty() || only.contains(&e.id)) && out.join(format!("{}.zip", e.id)).is_file())
        .map(|e| format!("{}.zip", e.id))
        .collect();
    println!("ml-publish: {} models in models.json, {} zip(s) to upload, in {}", m.models.len(), zips.len(), out.display());
    println!(
        "Upload (owner's go-ahead first; see docs/release-playbook.md), from {}: gh release upload {MODELS_TAG} --repo {MODELS_REPO} --clobber {} models.json MODELS-NOTICE.md",
        out.display(),
        zips.join(" ")
    );
    Ok(())
}

/// The release's current `models.json`, downloaded with `gh` into `out/live-models.json`.
fn download_live(out: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let path = out.join("live-models.json");
    let st = std::process::Command::new("gh")
        .args(["release", "download", MODELS_TAG, "--repo", MODELS_REPO, "--pattern", "models.json", "--clobber", "--output"])
        .arg(&path)
        .status()
        .map_err(|e| format!("gh: {e} (pass --merge <the release's models.json>, or --no-merge to write a manifest of only these models)"))?;
    if !st.success() {
        return Err(format!(
            "gh release download {MODELS_TAG} failed ({st}): pass --merge <the release's models.json>, or --no-merge to write a manifest of only these models"
        ));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_model(models: &Path, id: &str, onnx: &[u8], licence: &str) {
        let d = models.join(id);
        std::fs::create_dir_all(d.join("golden")).unwrap();
        std::fs::write(d.join("golden").join("input.f32"), b"test audio").unwrap();
        std::fs::write(d.join("m.onnx"), onnx).unwrap();
        let card = serde_json::json!({"id": id, "file": "m.onnx", "sha256": crate::ml_models::sha256(&d.join("m.onnx")).unwrap(),
            "licence": licence, "source_url": "https://example.invalid", "exported_utc": "2026-10-22T05:00:18Z"});
        std::fs::write(d.join("card.json"), card.to_string()).unwrap();
    }

    #[test]
    fn publish_writes_zips_whose_sha256_match_the_manifest() {
        let root = std::env::temp_dir().join(format!("xtask-ml-publish-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (models, out) = (root.join("models"), root.join("out"));
        fake_model(&models, "htdemucs", b"weights", "Code: MIT. Weights: released with the MIT repo.");
        fake_model(&models, "drumsep_mdx23c", b"drum weights", "Training code: MIT. Weights: no licence stated by the authors.");
        fake_model(&models, "srs_small", &[1u8; 2000], "MIT");
        let m = publish(&models, &out, "https://example.invalid/r", &[], 1000, None).unwrap();
        let read = Manifest::parse(&std::fs::read_to_string(out.join("models.json")).unwrap()).unwrap();
        assert_eq!(read, m);
        let ht = m.get("htdemucs").unwrap();
        assert!(ht.downloadable() && !ht.personal_use_only && ht.license == "MIT" && ht.version == "2026-10-22T05:00:18Z");
        assert_eq!(ht.url, "https://example.invalid/r/htdemucs.zip");
        for e in m.models.iter().filter(|e| !e.manual) {
            let z = out.join(format!("{}.zip", e.id));
            assert_eq!(crate::ml_models::sha256(&z).unwrap(), e.sha256);
            assert_eq!(std::fs::metadata(&z).unwrap().len(), e.size);
            let mut a = zip::ZipArchive::new(std::fs::File::open(&z).unwrap()).unwrap();
            let names: Vec<String> = (0..a.len()).map(|i| a.by_index(i).unwrap().name().unwrap().to_string()).collect();
            assert_eq!(names, ["card.json", "m.onnx"], "golden data is left out");
        }
        assert!(m.get("drumsep_mdx23c").unwrap().personal_use_only);
        // Over the asset limit (1000 bytes here), or not installed: install manually.
        assert!(m.get("srs_small").unwrap().manual && !out.join("srs_small.zip").exists());
        assert!(m.get("whisper_lyrics").unwrap().manual);
        assert!(m.get("bs_roformer_sw").unwrap().personal_use_only, "the catalogue's status when not installed");
        assert!(m.get("deepfilternet3").is_none(), "built in");
        let notice = std::fs::read_to_string(out.join("MODELS-NOTICE.md")).unwrap();
        assert!(notice.contains("MDX23C DrumSep") && notice.contains("personal use only"));
        // A file that no longer matches its card is refused.
        std::fs::write(models.join("htdemucs").join("m.onnx"), b"changed").unwrap();
        assert!(publish(&models, &out, "https://x/", &["htdemucs".into()], 1000, None).unwrap_err().contains("sha256"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn publishing_some_models_merges_into_the_release_manifest() {
        let root = std::env::temp_dir().join(format!("xtask-ml-publish-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (models, out) = (root.join("models"), root.join("out"));
        let old = |id: &str, sha: char| ManifestModel {
            id: id.into(),
            name: id.into(),
            version: "2026-10-01T00:00:00Z".into(),
            url: format!("https://x/{id}.zip"),
            size: 10,
            sha256: sha.to_string().repeat(64),
            license: "MIT".into(),
            ..Default::default()
        };
        let live = Manifest { format: MANIFEST_FORMAT, release: MODELS_TAG.into(), models: vec![old("htdemucs", 'a'), old("srs_small", 'b')] };
        fake_model(&models, "srs_small", b"new srs", "MIT");
        fake_model(&models, "basicpitch", b"pitch", "Apache-2.0");
        let only: Vec<String> = ["srs_small", "basicpitch", "btc_chords"].map(String::from).into();
        let m = publish(&models, &out, "https://x/", &only, 1 << 20, Some(&live)).unwrap();
        let ids: Vec<&str> = m.models.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["htdemucs", "srs_small", "basicpitch", "btc_chords"], "kept in place, replaced in place, then added");
        assert_eq!(m.models[0], live.models[0], "a model not packed is left as the release has it");
        assert_ne!(m.models[1], live.models[1]);
        assert_eq!(m.models[1].version, "2026-10-22T05:00:18Z");
        assert!(m.models[3].manual, "not installed and not in the release: install manually");
        let notice = std::fs::read_to_string(out.join("MODELS-NOTICE.md")).unwrap();
        assert!(notice.contains("(`htdemucs`)") && notice.contains("(`basicpitch`)"));
        // Requested but not installed while the release has it: the release's entry is kept.
        let m = publish(&models, &out, "https://x/", &["htdemucs".into()], 1 << 20, Some(&live)).unwrap();
        assert_eq!(m.models, live.models);
        let _ = std::fs::remove_dir_all(&root);
    }
}
