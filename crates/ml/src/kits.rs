//! Recorded drum kits for Juke, from the FreeDAWm kits release (`kits-v1` on GitHub).
//!
//! The release holds one zip per kit (the kit's folder: `kit.json` and its WAV files, see
//! `soundcraft_engine::juke_kits::RecordedKit`) and `kits.json` ([`KitsManifest`]). As with the
//! models, nothing runs on its own: the app calls this after a click, on a worker thread. A
//! download is checked against the manifest's size and sha256, unpacked into a staging folder,
//! checked for its `kit.json`, and only then swapped in as `<kits>/<id>`.

use crate::catalog::{ManifestModel, resolve_url};
use crate::download::{DownloadProgress, fetch_zip, io, recover, swap, unpack};
use crate::job::Cancel;
use crate::{MlError, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Where the app reads the kits manifest (`FREEDAWM_KITS_URL` overrides it).
pub const KITS_MANIFEST_URL: &str = "https://github.com/s0lidr0ck/FreeDAWm/releases/download/kits-v1/kits.json";
/// The largest kits manifest read (bytes).
const MAX_MANIFEST_BYTES: u64 = 1 << 20;

/// `kits.json`: what the kits release holds.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct KitsManifest {
    #[serde(default)]
    pub format: u32,
    #[serde(default)]
    pub release: String,
    #[serde(default)]
    pub kits: Vec<KitEntry>,
}

/// One kit of the release: a zip of its folder.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct KitEntry {
    /// Its folder's name (letters, digits, `-` and `_`).
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// The licence, as checked on the kit's own download page.
    #[serde(default)]
    pub license: String,
    /// Who made it, and where it came from.
    #[serde(default)]
    pub credit: String,
    /// The zip: absolute, or relative to the manifest's URL.
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub size: u64,
    /// The zip's sha256, lowercase hex.
    #[serde(default)]
    pub sha256: String,
}

impl KitEntry {
    /// Whether the app can download it.
    pub fn downloadable(&self) -> bool {
        valid_id(&self.id) && !self.url.is_empty() && self.size > 0 && self.sha256.len() == 64
    }
}

impl KitsManifest {
    /// Reads `kits.json`, dropping entries that cannot be downloaded.
    pub fn parse(text: &str) -> std::result::Result<KitsManifest, String> {
        let mut m: KitsManifest = serde_json::from_str(text).map_err(|e| format!("kits.json: {e}"))?;
        m.kits.retain(KitEntry::downloadable);
        Ok(m)
    }
}

/// A kit id names one plain folder.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The manifest's URL: `FREEDAWM_KITS_URL` when set, else the release's `kits.json`.
pub fn kits_manifest_url() -> String {
    crate::env_var_os("KITS_URL")
        .and_then(|v| v.into_string().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| KITS_MANIFEST_URL.to_string())
}

/// The folder kits install into: `FREEDAWM_KITS_DIR` when set, else `kits` beside the models.
pub fn kits_dir() -> PathBuf {
    if let Some(d) = crate::env_var_os("KITS_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    crate::models::default_models_dir().with_file_name("kits")
}

/// Fetches and reads the kits manifest.
pub fn fetch_kits_manifest(url: &str) -> Result<KitsManifest> {
    let resp = ureq::get(url).call().map_err(|e| MlError::Io(format!("can't read {url}: {e}")))?;
    let mut text = String::new();
    resp.into_reader().take(MAX_MANIFEST_BYTES + 1).read_to_string(&mut text).map_err(|e| MlError::Io(format!("{url}: {e}")))?;
    KitsManifest::parse(&text).map_err(MlError::Io)
}

/// Downloads kit `k` from the release whose manifest is at `manifest_url` and installs it into
/// `dir` as `<dir>/<id>`. Cancelling stops at the next block and cleans up.
pub fn download_kit(dir: &Path, k: &KitEntry, manifest_url: &str, cancel: &Cancel, progress: &DownloadProgress) -> Result<()> {
    if !k.downloadable() {
        return Err(MlError::BadInput(format!("`{}` is not a kit FreeDAWm downloads", k.id)));
    }
    let downloads = dir.join(".downloads");
    std::fs::create_dir_all(&downloads).map_err(|e| io(&downloads, e))?;
    recover(dir, &k.id);
    let part = downloads.join(format!("{}.zip.part", k.id));
    let as_model = ManifestModel { id: k.id.clone(), url: k.url.clone(), size: k.size, sha256: k.sha256.clone(), ..Default::default() };
    let r = fetch_zip(&resolve_url(manifest_url, &k.url), &as_model, &part, cancel, progress).and_then(|()| install_kit_zip(dir, &k.id, &part));
    let _ = std::fs::remove_file(&part);
    let _ = std::fs::remove_dir(&downloads);
    r
}

/// Unpacks a kit zip (its `kit.json` and samples at the zip's root) into `dir` as kit `id` and
/// swaps it in. A broken zip, or one without a `kit.json`, leaves `dir/<id>` as it was.
pub fn install_kit_zip(dir: &Path, id: &str, zip_path: &Path) -> Result<()> {
    if !valid_id(id) {
        return Err(MlError::BadInput(format!("`{id}` is not a kit id")));
    }
    recover(dir, id);
    let staging = dir.join(format!(".{id}.staging"));
    let r = unpack(zip_path, &staging.join(id)).and_then(|()| {
        if !staging.join(id).join("kit.json").is_file() {
            return Err(MlError::Io(format!("{}: not a kit (no kit.json)", zip_path.display())));
        }
        swap(dir, id, &staging.join(id))
    });
    let _ = std::fs::remove_dir_all(&staging);
    r
}

/// The kits installed in `dir`: (id, folder), sorted by id.
pub fn installed_kits(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<(String, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let path = e.path();
            (valid_id(&name) && path.join("kit.json").is_file()).then_some((name, path))
        })
        .collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fdawm-kits-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn zip_of(path: &Path, files: &[(&str, &[u8])]) {
        let f = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        for (name, bytes) in files {
            z.start_file(*name, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();
            z.write_all(bytes).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn the_manifest_keeps_only_downloadable_kits() {
        let good = format!(r#"{{"id":"sm_drums","name":"SM Drums","url":"sm_drums.zip","size":10,"sha256":"{}"}}"#, "a".repeat(64));
        let text = format!(
            r#"{{"format":1,"release":"kits-v1","kits":[{good},{{"id":"../x","url":"x.zip","size":1,"sha256":"{}"}},{{"id":"empty"}}]}}"#,
            "b".repeat(64)
        );
        let m = KitsManifest::parse(&text).unwrap();
        assert_eq!(m.kits.iter().map(|k| k.id.as_str()).collect::<Vec<_>>(), ["sm_drums"]);
        assert!(KitsManifest::parse("nope").is_err());
        assert!(!valid_id("a/b") && !valid_id("") && valid_id("SM-Drums_1"));
    }

    #[test]
    fn a_kit_zip_installs_and_lists_and_a_bad_one_changes_nothing() {
        let d = tmp("install");
        let z = d.join("k.zip");
        zip_of(&z, &[("kit.json", br#"{"name":"Test"}"#), ("kick/v1r1.wav", b"RIFF")]);
        let kits = d.join("kits");
        install_kit_zip(&kits, "test_kit", &z).unwrap();
        assert_eq!(installed_kits(&kits).iter().map(|(i, _)| i.as_str()).collect::<Vec<_>>(), ["test_kit"]);
        assert!(kits.join("test_kit").join("kick").join("v1r1.wav").is_file());
        // No kit.json: refused, the installed kit kept.
        zip_of(&z, &[("readme.txt", b"hi")]);
        assert!(install_kit_zip(&kits, "test_kit", &z).is_err());
        assert!(kits.join("test_kit").join("kit.json").is_file());
        assert!(install_kit_zip(&kits, "../evil", &z).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Serves `files` (path, bytes) over HTTP on localhost for `n` requests; returns the base URL.
    fn serve(files: Vec<(String, Vec<u8>)>, n: usize) -> String {
        use std::io::{BufRead, BufReader};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in l.incoming().take(n).flatten() {
                let mut s = stream;
                let mut first = String::new();
                let mut r = BufReader::new(s.try_clone().unwrap());
                r.read_line(&mut first).unwrap();
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap() == 0 || h == "\r\n" {
                        break;
                    }
                }
                let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                match files.iter().find(|(p, _)| *p == path) {
                    Some((_, body)) => {
                        let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                        let _ = s.write_all(body);
                    }
                    None => {
                        let _ = write!(s, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                }
            }
        });
        base
    }

    #[test]
    fn a_kit_downloads_from_the_release_list_and_installs() {
        use sha2::Digest;
        let d = tmp("http");
        let z = d.join("src.zip");
        zip_of(&z, &[("kit.json", br#"{"name":"Served"}"#), ("a.wav", b"RIFF")]);
        let bytes = std::fs::read(&z).unwrap();
        let sha: String = sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
        let list = format!(
            r#"{{"format":1,"release":"kits-v1","kits":[{{"id":"served","name":"Served","url":"served.zip","size":{},"sha256":"{sha}"}}]}}"#,
            bytes.len()
        );
        let base = serve(vec![("/kits.json".into(), list.into_bytes()), ("/served.zip".into(), bytes.clone())], 2);
        let url = format!("{base}/kits.json");
        let m = fetch_kits_manifest(&url).unwrap();
        assert_eq!(m.kits.len(), 1);
        let kits = d.join("kits");
        download_kit(&kits, &m.kits[0], &url, &Cancel::default(), &DownloadProgress::new()).unwrap();
        assert_eq!(installed_kits(&kits).len(), 1);
        assert!(!kits.join(".downloads").exists(), "the partial download is cleaned up");
        // A zip that does not match its sha256 is refused.
        let mut wrong = m.kits[0].clone();
        wrong.id = "other".into();
        wrong.sha256 = "0".repeat(64);
        let base = serve(vec![("/served.zip".into(), bytes)], 1);
        assert!(download_kit(&kits, &wrong, &format!("{base}/kits.json"), &Cancel::default(), &DownloadProgress::new()).is_err());
        assert!(!kits.join("other").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_download_checks_the_entry_first() {
        let d = tmp("dl");
        let k = KitEntry { id: "x".into(), url: String::new(), ..Default::default() };
        let cancel = Cancel::default();
        assert!(download_kit(&d, &k, KITS_MANIFEST_URL, &cancel, &DownloadProgress::new()).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
