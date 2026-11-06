//! `cargo xtask kit-package <folder> [--id ID] [--out DIR] [--base-url URL]` packs a recorded
//! Juke kit for the kits release (`kits-v1`): checks the folder's `kit.json` (a name, at most 16
//! pads, sample keys `v1.r1`…`v4.r4`, every sample file present and inside the folder), zips the
//! folder (stored entries) as `<out>/<id>.zip`, and writes or updates `<out>/kits.json` with the
//! kit's entry (size, sha256, licence and credit from `kit.json`). Upload both files to the release.
//! Before packing a kit, check its licence on its own download page and put it in `kit.json`.

use std::io::Write;
use std::path::{Path, PathBuf};

const PADS: usize = 16;

fn files(dir: &Path, base: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let e = e.map_err(|e| e.to_string())?;
        let p = e.path();
        let ty = e.file_type().map_err(|e| e.to_string())?;
        if ty.is_symlink() {
            return Err(format!("{}: links are not packed", p.display()));
        }
        if ty.is_dir() {
            files(&p, base, out)?;
        } else if let Ok(rel) = p.strip_prefix(base) {
            out.push(rel.to_path_buf());
        }
    }
    Ok(())
}

/// The kit's name, licence and credit, after checking `kit.json` against the folder.
fn check(dir: &Path) -> Result<(String, String, String), String> {
    let path = dir.join("kit.json");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let s = |k: &str| v.get(k).and_then(serde_json::Value::as_str).unwrap_or("").trim().to_string();
    let name = s("name");
    if name.is_empty() {
        return Err("kit.json: no `name`".into());
    }
    if s("license").is_empty() {
        return Err("kit.json: no `license` (check it on the kit's download page)".into());
    }
    let pads = v.get("pads").and_then(serde_json::Value::as_array).ok_or("kit.json: no `pads`")?;
    if pads.is_empty() || pads.len() > PADS {
        return Err(format!("kit.json: {} pads (1 to {PADS})", pads.len()));
    }
    for (i, pad) in pads.iter().enumerate() {
        let samples = pad.get("samples").and_then(serde_json::Value::as_object).ok_or(format!("pad {}: no `samples`", i + 1))?;
        for (key, file) in samples {
            let ok_key = key.split_once('.').is_some_and(|(a, b)| {
                let n = |x: &str, p: char| x.strip_prefix(p).and_then(|x| x.parse::<usize>().ok()).is_some_and(|n| (1..=4).contains(&n));
                n(a, 'v') && n(b, 'r')
            });
            let file = file.as_str().unwrap_or("");
            let plain = !file.is_empty() && Path::new(file).components().all(|c| matches!(c, std::path::Component::Normal(_)));
            if !ok_key || !plain || !dir.join(file).is_file() {
                return Err(format!("pad {}: `{key}` -> `{file}` is not a layer/take and a file in the folder", i + 1));
            }
        }
    }
    Ok((name, s("license"), s("credit")))
}

pub fn run(_root: &Path, args: &[String]) -> Result<(), String> {
    let mut dir = None;
    let (mut id, mut out, mut base) = (None, PathBuf::from("target/kits-release"), String::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--id" => id = it.next().cloned(),
            "--out" => out = it.next().map(PathBuf::from).ok_or("--out needs a folder")?,
            "--base-url" => base = it.next().cloned().ok_or("--base-url needs a URL")?,
            x if dir.is_none() => dir = Some(PathBuf::from(x)),
            x => return Err(format!("kit-package: unexpected `{x}`")),
        }
    }
    let dir = dir.ok_or("usage: cargo xtask kit-package <folder> [--id ID] [--out DIR] [--base-url URL]")?;
    let id = id.unwrap_or_else(|| dir.file_name().and_then(|n| n.to_str()).unwrap_or("kit").to_string());
    if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("`{id}`: a kit id is letters, digits, `-` and `_`"));
    }
    let (name, license, credit) = check(&dir)?;
    let mut list = Vec::new();
    files(&dir, &dir, &mut list)?;
    list.sort();
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let zip_path = out.join(format!("{id}.zip"));
    let f = std::fs::File::create(&zip_path).map_err(|e| format!("{}: {e}", zip_path.display()))?;
    let mut w = zip::ZipWriter::new(std::io::BufWriter::new(f));
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored).large_file(true);
    for rel in &list {
        let name = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
        w.start_file(name, opts).map_err(|e| e.to_string())?;
        let bytes = std::fs::read(dir.join(rel)).map_err(|e| format!("{}: {e}", rel.display()))?;
        w.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    w.finish().map_err(|e| e.to_string())?;
    let bytes = std::fs::read(&zip_path).map_err(|e| e.to_string())?;
    use sha2::Digest;
    let sha: String = sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    let url = if base.is_empty() { format!("{id}.zip") } else { format!("{}/{id}.zip", base.trim_end_matches('/')) };
    let entry = serde_json::json!({"id": id, "name": name, "license": license, "credit": credit, "url": url, "size": bytes.len(), "sha256": sha});
    let manifest_path = out.join("kits.json");
    let mut manifest: serde_json::Value = std::fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({"format": 1, "release": "kits-v1", "kits": []}));
    if let Some(kits) = manifest.get_mut("kits").and_then(serde_json::Value::as_array_mut) {
        kits.retain(|k| k.get("id").and_then(serde_json::Value::as_str) != Some(id.as_str()));
        kits.push(entry);
    }
    let text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    std::fs::write(&manifest_path, text + "\n").map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    println!(
        "kit-package: {} ({} files, {} bytes) -> {}; upload it and kits.json to the kits-v1 release",
        name,
        list.len(),
        bytes.len(),
        zip_path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kit_folder_packs_and_a_bad_one_is_refused() {
        let d = std::env::temp_dir().join(format!("xtask-kit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let kit = d.join("test_kit");
        std::fs::create_dir_all(kit.join("kick")).unwrap();
        std::fs::write(kit.join("kick").join("v1r1.wav"), b"RIFF").unwrap();
        std::fs::write(kit.join("kit.json"), r#"{"name":"T","license":"CC0","pads":[{"name":"K","samples":{"v1.r1":"kick/v1r1.wav"}}]}"#).unwrap();
        let out = d.join("out");
        run(&d, &[kit.to_string_lossy().into_owned(), "--out".into(), out.to_string_lossy().into_owned()]).unwrap();
        let m: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out.join("kits.json")).unwrap()).unwrap();
        assert_eq!(m["kits"][0]["id"], "test_kit");
        assert_eq!(m["kits"][0]["sha256"].as_str().unwrap().len(), 64);
        std::fs::write(kit.join("kit.json"), r#"{"name":"T","license":"CC0","pads":[{"name":"K","samples":{"v1.r1":"nope.wav"}}]}"#).unwrap();
        assert!(run(&d, &[kit.to_string_lossy().into_owned(), "--out".into(), out.to_string_lossy().into_owned()]).is_err());
        std::fs::write(kit.join("kit.json"), r#"{"name":"T","pads":[{"name":"K","samples":{}}]}"#).unwrap();
        assert!(check(&kit).is_err(), "no licence");
        let _ = std::fs::remove_dir_all(&d);
    }
}
