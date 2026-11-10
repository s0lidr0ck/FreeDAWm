//! Downloading models from the FreeDAWm models release (spec 2026-11-04-model-downloads).
//!
//! The release (`models-v1` on GitHub) holds one zip per model, the model's folder as
//! `cargo xtask ml-models` installs it, and `models.json` ([`Manifest`]). Nothing here runs on its
//! own: the app calls it after a click, on a worker thread.
//!
//! A download goes to `<models>/.downloads/<id>.zip.part`, hashed as it arrives; a wrong size or
//! sha256 deletes it. The zip is unpacked into `<models>/.<id>.staging/<id>/`, its card read and
//! every file checked against the card's sha256 ([`crate::models::load`]); only then is the folder
//! swapped in (the old one renamed to `.<id>.old`, the new one renamed into place, the old one
//! deleted). The listing only looks at `<models>/<id>`, so an interrupted download never reads as
//! installed, and a failed swap puts the old folder back. A retry starts again from zero.

use crate::catalog::{MAX_MANIFEST_BYTES, Manifest, ManifestModel, resolve_url};
use crate::job::Cancel;
use crate::{MlError, Result};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The largest model folder unpacked (bytes): the biggest model today is under 1 GB.
const MAX_UNPACKED: u64 = 8 * 1024 * 1024 * 1024;
/// The most entries one model zip may hold.
const MAX_ENTRIES: usize = 4096;

/// The manifest's URL: `FREEDAWM_MODELS_URL` (or `SOUNDCRAFT_MODELS_URL`) when set, else the
/// release's `models.json`.
pub fn manifest_url() -> String {
    crate::env_var_os("MODELS_URL")
        .and_then(|v| v.into_string().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| crate::catalog::MANIFEST_URL.to_string())
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(20))
        .timeout_read(Duration::from_secs(60))
        .user_agent(concat!("FreeDAWm/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn get(url: &str) -> Result<ureq::Response> {
    match agent().get(url).call() {
        Ok(r) => Ok(r),
        Err(ureq::Error::Status(code, _)) => Err(MlError::Io(format!("{url}: the server answered {code}"))),
        Err(ureq::Error::Transport(t)) => Err(MlError::Io(format!("can't reach {url}: {t}"))),
    }
}

/// Fetches and reads the manifest, with the song engines' (`engines.json` beside it, when the
/// release has one) merged in, every entry resolved for this computer's platform.
pub fn fetch_manifest(url: &str) -> Result<Manifest> {
    let read = |u: &str| -> Result<Manifest> {
        let mut text = String::new();
        get(u)?.into_reader().take(MAX_MANIFEST_BYTES + 1).read_to_string(&mut text).map_err(|e| MlError::Io(format!("{u}: {e}")))?;
        Manifest::parse(&text).map_err(MlError::Io)
    };
    let mut m = read(url)?;
    let engines = resolve_url(url, crate::catalog::ENGINES_MANIFEST);
    match read(&engines) {
        Ok(e) => m.merge(e),
        // No engines published yet (or unreachable): the models alone.
        Err(e) => log::info!("{engines}: {e}"),
    }
    Ok(m.for_platform(&crate::catalog::current_platform()))
}

/// Bytes received of bytes expected, shared between the worker and the UI.
#[derive(Debug, Default)]
pub struct DownloadProgress {
    done: AtomicU64,
    total: AtomicU64,
    /// 0 downloading, 1 checking and unpacking.
    phase: AtomicU64,
}

impl DownloadProgress {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// (received, expected) bytes.
    pub fn get(&self) -> (u64, u64) {
        (self.done.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed))
    }

    /// Whether the download is done and the zip is being unpacked and checked.
    pub fn installing(&self) -> bool {
        self.phase.load(Ordering::Relaxed) == 1
    }

    /// For renders and tests: a download `done` of `total` bytes in.
    pub fn set(&self, done: u64, total: u64) {
        self.done.store(done, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }
}

pub(crate) fn io(p: &Path, e: impl std::fmt::Display) -> MlError {
    MlError::Io(format!("{}: {e}", p.display()))
}

fn known(id: &str) -> Result<&'static crate::catalog::KnownModel> {
    crate::catalog::KNOWN_MODELS
        .iter()
        .find(|m| m.id == id && !m.builtin)
        .ok_or_else(|| MlError::BadInput(format!("`{id}` is not a model FreeDAWm downloads")))
}

/// Downloads model `m` from the release whose manifest is at `manifest_url` and installs it into
/// `dir` (see the module notes). Cancelling stops at the next block and cleans up.
pub fn download(dir: &Path, m: &ManifestModel, manifest_url: &str, cancel: &Cancel, progress: &DownloadProgress) -> Result<()> {
    known(&m.id)?;
    if !m.downloadable() {
        return Err(MlError::BadInput(format!("`{}` is not in the models release: {}", m.id, m.manual_note)));
    }
    let downloads = dir.join(".downloads");
    std::fs::create_dir_all(&downloads).map_err(|e| io(&downloads, e))?;
    recover(dir, &m.id);
    let part = downloads.join(format!("{}.zip.part", m.id));
    progress.set(0, m.total_size());
    let r = fetch_to(&resolve_url(manifest_url, &m.url), &m.id, m.size, &m.sha256, &part, cancel, progress, 0).and_then(|()| {
        install_zip_with(dir, &m.id, &part, |folder| {
            // The files fetched from their authors' pages (song engines' models).
            let mut base = m.size;
            for f in &m.fetch {
                let dest = folder.join(&f.file);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
                }
                fetch_to(&f.url, &m.id, f.size, &f.sha256, &dest, cancel, progress, base)?;
                base = base.saturating_add(f.size);
            }
            progress.phase.store(1, Ordering::Relaxed);
            Ok(())
        })
    });
    let _ = std::fs::remove_file(&part);
    let _ = std::fs::remove_dir(&downloads);
    r
}

/// Streams `url` into `part`, checking its size and sha256 against `m`.
pub(crate) fn fetch_zip(url: &str, m: &ManifestModel, part: &Path, cancel: &Cancel, progress: &DownloadProgress) -> Result<()> {
    progress.set(0, m.size);
    fetch_to(url, &m.id, m.size, &m.sha256, part, cancel, progress, 0)
}

/// Streams `url` into `part` (for model `id`), checking it is `size` bytes with `sha256`; progress
/// counts from `base`.
#[allow(clippy::too_many_arguments)]
fn fetch_to(url: &str, id: &str, size: u64, sha256: &str, part: &Path, cancel: &Cancel, progress: &DownloadProgress, base: u64) -> Result<()> {
    use sha2::Digest;
    let mut reader = get(url)?.into_reader().take(size.saturating_add(1));
    let mut out = std::fs::File::create(part).map_err(|e| io(part, e))?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        if cancel.is_cancelled() {
            return Err(MlError::Cancelled);
        }
        let n = reader.read(&mut buf).map_err(|e| MlError::Io(format!("downloading {id}: {e}")))?;
        if n == 0 {
            break;
        }
        let chunk = buf.get(..n).unwrap_or_default();
        h.update(chunk);
        out.write_all(chunk).map_err(|e| io(part, e))?;
        done = done.saturating_add(n as u64);
        progress.done.store(base.saturating_add(done), Ordering::Relaxed);
    }
    out.flush().map_err(|e| io(part, e))?;
    drop(out);
    if done != size {
        return Err(MlError::Io(format!("{id}: the download stopped at {done} of {size} bytes; try again")));
    }
    let got: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if !got.eq_ignore_ascii_case(sha256) {
        let _ = std::fs::remove_file(part);
        return Err(MlError::Model(id.into(), format!("the download does not match the release (sha256 {got}, expected {sha256}); deleted")));
    }
    Ok(())
}

/// Puts a model folder back after a swap that stopped half way, and clears old leftovers.
pub(crate) fn recover(dir: &Path, id: &str) {
    let (live, old) = (dir.join(id), dir.join(format!(".{id}.old")));
    if std::fs::symlink_metadata(&old).is_ok() {
        if std::fs::symlink_metadata(&live).is_err() {
            let _ = std::fs::rename(&old, &live);
        } else {
            let _ = std::fs::remove_dir_all(&old);
        }
    }
    let _ = std::fs::remove_dir_all(dir.join(format!(".{id}.staging")));
    let _ = std::fs::remove_dir_all(dir.join(format!(".{id}.removing")));
}

/// Unpacks a model zip (entries at its root: `card.json`, the ONNX and extra files) into `dir`
/// as model `id`, checked against its card, and swaps it in. A broken zip or a file that does
/// not match the card leaves `dir/<id>` as it was.
pub fn install_zip(dir: &Path, id: &str, zip_path: &Path) -> Result<()> {
    install_zip_with(dir, id, zip_path, |_| Ok(()))
}

/// [`install_zip`], with `more` adding files to the unpacked folder before it is checked.
fn install_zip_with(dir: &Path, id: &str, zip_path: &Path, more: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    known(id)?;
    recover(dir, id);
    let staging = dir.join(format!(".{id}.staging"));
    let r = unpack(zip_path, &staging.join(id)).and_then(|()| more(&staging.join(id))).and_then(|()| {
        let card = crate::models::load(&staging, id)?;
        if card.card.id != id {
            return Err(MlError::Model(id.into(), format!("the zip holds model `{}`", card.card.id)));
        }
        swap(dir, id, &staging.join(id))
    });
    let _ = std::fs::remove_dir_all(&staging);
    r
}

pub(crate) fn unpack(zip_path: &Path, to: &Path) -> Result<()> {
    let bad = |e: &dyn std::fmt::Display| MlError::Io(format!("{}: not a model zip ({e})", zip_path.display()));
    let f = std::fs::File::open(zip_path).map_err(|e| io(zip_path, e))?;
    let mut z = zip::ZipArchive::new(std::io::BufReader::new(f)).map_err(|e| bad(&e))?;
    if z.len() > MAX_ENTRIES {
        return Err(bad(&"too many files"));
    }
    std::fs::create_dir_all(to).map_err(|e| io(to, e))?;
    let mut total = 0u64;
    for i in 0..z.len() {
        let mut entry = z.by_index(i).map_err(|e| bad(&e))?;
        let label = entry.name().map(|n| n.to_string()).unwrap_or_default();
        let Some(name) = entry.enclosed_name() else {
            return Err(bad(&format!("`{}` points outside the folder", label)));
        };
        if !name.components().all(|c| matches!(c, std::path::Component::Normal(_))) || entry.is_symlink() {
            return Err(bad(&format!("`{}` is not a plain file", label)));
        }
        let path = to.join(&name);
        if entry.is_dir() {
            std::fs::create_dir_all(&path).map_err(|e| io(&path, e))?;
            continue;
        }
        total = total.saturating_add(entry.size());
        if total > MAX_UNPACKED {
            return Err(bad(&"larger than any model"));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
        }
        let mut out = std::fs::File::create(&path).map_err(|e| io(&path, e))?;
        let size = entry.size();
        let n = std::io::copy(&mut (&mut entry).take(size), &mut out).map_err(|e| bad(&e))?;
        if n != size {
            return Err(bad(&format!("`{}` is cut short", label)));
        }
    }
    Ok(())
}

/// Swaps the checked folder `new` in as `dir/<id>`.
pub(crate) fn swap(dir: &Path, id: &str, new: &Path) -> Result<()> {
    let (live, old) = (dir.join(id), dir.join(format!(".{id}.old")));
    let had = std::fs::symlink_metadata(&live).is_ok();
    if had {
        std::fs::rename(&live, &old).map_err(|e| MlError::Io(format!("{} is in use ({e}): close what uses it and try again", live.display())))?;
    }
    if let Err(e) = std::fs::rename(new, &live) {
        if had {
            let _ = std::fs::rename(&old, &live);
        }
        return Err(io(&live, e));
    }
    if had {
        let _ = std::fs::remove_dir_all(&old);
    }
    Ok(())
}

/// Removes model `id` from `dir` (renamed aside first, so a half-deleted folder never reads as
/// installed).
pub fn remove(dir: &Path, id: &str) -> Result<()> {
    known(id)?;
    let live = dir.join(id);
    if std::fs::symlink_metadata(&live).is_err() {
        return Err(MlError::NotInstalled(format!("`{id}` is not installed in {}", dir.display())));
    }
    let gone: PathBuf = dir.join(format!(".{id}.removing"));
    let _ = std::fs::remove_dir_all(&gone);
    std::fs::rename(&live, &gone).map_err(|e| MlError::Io(format!("{} is in use ({e}): close what uses it and try again", live.display())))?;
    // A link (the model kept elsewhere) goes; its target stays.
    if std::fs::symlink_metadata(&gone).is_ok_and(|m| m.file_type().is_symlink()) {
        return std::fs::remove_dir(&gone).or_else(|_| std::fs::remove_file(&gone)).map_err(|e| io(&gone, e));
    }
    std::fs::remove_dir_all(&gone).map_err(|e| io(&gone, e))
}

/// The installed card's version (`version`, else `exported_utc`), empty when it has none.
pub fn card_version(card: &crate::models::ModelCard) -> String {
    card.extra.get("version").or_else(|| card.extra.get("exported_utc")).and_then(serde_json::Value::as_str).unwrap_or("").to_string()
}
