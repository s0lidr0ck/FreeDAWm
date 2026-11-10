//! The model folder and model cards.
//!
//! Models are not bundled: Setup › ML Models downloads them from the FreeDAWm models release
//! ([`crate::download`]), or `cargo xtask ml-models` exports them and installs each into
//! `<models dir>/<id>/` with a `card.json` (id, name, sample rate, stems, source, licence, the
//! ONNX file name and its sha256). The folder is `%LOCALAPPDATA%\FreeDAWm\models` on Windows
//! (`~/Library/Application Support/FreeDAWm/models` on macOS, `$XDG_DATA_HOME/FreeDAWm/models`
//! elsewhere), or `FREEDAWM_MODELS_DIR` (the older `SOUNDCRAFT_MODELS_DIR` still works) when set.
//! Until the new folder exists, the pre-rename `SoundCraft/models` folder is used when it exists:
//! models are large, so they are not copied. A model's files are checked against the card's
//! sha256 the first time they are loaded in a run of the app.

use crate::{MlError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The environment variable that overrides the model folder.
pub const MODELS_DIR_ENV: &str = "FREEDAWM_MODELS_DIR";

/// The largest card.json read (bytes): a card is a few hundred bytes of JSON.
const MAX_CARD_BYTES: u64 = 1 << 20;

/// The model folder.
pub fn models_dir() -> PathBuf {
    if let Some(d) = crate::env_var_os("MODELS_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    default_models_dir()
}

/// The model folder when `FREEDAWM_MODELS_DIR` is not set.
pub fn default_models_dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .filter(|d| !d.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
    };
    let base = base.unwrap_or_else(std::env::temp_dir);
    pick_models_dir(base.join("FreeDAWm").join("models"), base.join("SoundCraft").join("models"))
}

/// The new model folder, unless only the pre-rename one exists.
fn pick_models_dir(new: PathBuf, legacy: PathBuf) -> PathBuf {
    if !new.exists() && legacy.is_dir() { legacy } else { new }
}

/// One extra file a model needs (external weights), with its checksum.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtraFile {
    pub file: String,
    #[serde(default)]
    pub sha256: String,
}

/// A model card (`<id>/card.json`). Only `file` is required; runner-specific settings live in
/// `extra` and are read by the runner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelCard {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// The ONNX file, relative to the model's folder.
    pub file: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default)]
    pub sample_rate: u32,
    #[serde(default)]
    pub channels: u32,
    #[serde(default)]
    pub stems: Vec<String>,
    #[serde(default, alias = "source_url", alias = "url")]
    pub source: String,
    #[serde(default, alias = "licence")]
    pub license: String,
    #[serde(default)]
    pub extra_files: Vec<ExtraFile>,
    /// Everything else on the card (chunking, STFT parameters, input/output names…).
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl ModelCard {
    /// A number from the card (top level or under `"params"`), or `d`.
    pub fn num(&self, key: &str, d: f64) -> f64 {
        self.value(key).and_then(serde_json::Value::as_f64).filter(|v| v.is_finite()).unwrap_or(d)
    }

    /// A string from the card (top level or under `"params"`).
    pub fn text(&self, key: &str) -> Option<&str> {
        self.value(key).and_then(serde_json::Value::as_str)
    }

    /// A value from the card: top level, or nested under `"params"`, `"stft"` or `"chunk"`.
    pub fn value(&self, key: &str) -> Option<&serde_json::Value> {
        self.extra.get(key).or_else(|| ["params", "stft", "chunk", "chunking", "io"].iter().find_map(|g| self.extra.get(*g).and_then(|v| v.get(key))))
    }
}

pub use crate::catalog::{KNOWN_MODELS, KnownModel, LICENCE_NOT_STATED, known_licence, licence_status};

/// How a model stands in the model folder.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelStatus {
    Installed(Box<ModelCard>),
    /// Built into the app.
    Builtin,
    Missing,
    /// There, but unusable (unreadable card, missing ONNX file); the reason.
    Broken(String),
}

/// A row of the Models page.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelEntry {
    pub model: KnownModel,
    pub status: ModelStatus,
    /// Bytes on disk (ONNX file plus extra files), 0 when missing.
    pub size: u64,
    pub dir: PathBuf,
}

impl ModelEntry {
    /// The licence status to show: from the installed card's `license` field, else the one
    /// known for the model.
    pub fn licence(&self) -> String {
        match &self.status {
            ModelStatus::Installed(card) => licence_status(&card.license).unwrap_or_else(|| self.model.license.to_string()),
            _ => self.model.license.to_string(),
        }
    }
}

/// Every known model and how it stands in `dir`.
pub fn list(dir: &Path) -> Vec<ModelEntry> {
    KNOWN_MODELS
        .iter()
        .map(|m| {
            let mdir = dir.join(m.id);
            if m.builtin {
                return ModelEntry { model: *m, status: ModelStatus::Builtin, size: 0, dir: mdir };
            }
            let (status, size) = match read_card(dir, m.id) {
                Ok(card) => {
                    let files: Vec<PathBuf> =
                        std::iter::once(mdir.join(&card.file)).chain(card.extra_files.iter().map(|f| mdir.join(&f.file))).collect();
                    match files.iter().find(|f| !f.is_file()) {
                        Some(f) => (ModelStatus::Broken(format!("{} is missing", f.display())), 0),
                        None => {
                            let size = files.iter().filter_map(|f| std::fs::metadata(f).ok()).map(|m| m.len()).sum();
                            (ModelStatus::Installed(Box::new(card)), size)
                        }
                    }
                }
                Err(MlError::NotInstalled(_)) => (ModelStatus::Missing, 0),
                Err(e) => (ModelStatus::Broken(e.to_string()), 0),
            };
            ModelEntry { model: *m, status, size, dir: mdir }
        })
        .collect()
}

/// What to tell the person about a missing model.
pub fn how_to_install(id: &str, dir: &Path) -> String {
    format!("the `{id}` model is not installed in {}: download it in Setup > ML Models (or run `cargo xtask ml-models`)", dir.display())
}

/// `<dir>/<id>/card.json`.
pub fn read_card(dir: &Path, id: &str) -> Result<ModelCard> {
    if id.is_empty() || id.contains(['/', '\\', '.']) {
        return Err(MlError::BadInput(format!("bad model id `{id}`")));
    }
    let path = dir.join(id).join("card.json");
    let meta = std::fs::metadata(&path).map_err(|_| MlError::NotInstalled(how_to_install(id, dir)))?;
    if !meta.is_file() || meta.len() > MAX_CARD_BYTES {
        return Err(MlError::Model(id.into(), format!("{} is not a model card", path.display())));
    }
    let text = std::fs::read_to_string(&path).map_err(|e| MlError::Io(format!("{}: {e}", path.display())))?;
    let mut card: ModelCard = serde_json::from_str(&text).map_err(|e| MlError::Model(id.into(), format!("card.json: {e}")))?;
    if card.id.is_empty() {
        card.id = id.into();
    }
    for f in std::iter::once(card.file.as_str()).chain(card.extra_files.iter().map(|f| f.file.as_str())) {
        // Card file names stay inside the model's folder.
        let p = Path::new(f);
        if f.is_empty() || p.is_absolute() || p.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
            return Err(MlError::Model(id.into(), format!("card.json names a file outside the model folder: `{f}`")));
        }
    }
    Ok(card)
}

/// An installed, checked model: its card and folder.
#[derive(Debug, Clone, PartialEq)]
pub struct InstalledModel {
    pub card: ModelCard,
    pub dir: PathBuf,
}

impl InstalledModel {
    /// The ONNX file.
    pub fn onnx(&self) -> PathBuf {
        self.dir.join(&self.card.file)
    }

    /// Another file in the model's folder (`name` relative to it, never outside).
    pub fn file(&self, name: &str) -> Result<PathBuf> {
        let p = Path::new(name);
        if name.is_empty() || p.is_absolute() || p.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
            return Err(MlError::Model(self.card.id.clone(), format!("`{name}` is not a file in the model folder")));
        }
        Ok(self.dir.join(p))
    }
}

/// Files already checked in this run of the app: (path, size, modified).
fn verified() -> &'static Mutex<HashSet<(PathBuf, u64, Option<std::time::SystemTime>)>> {
    static V: std::sync::OnceLock<Mutex<HashSet<(PathBuf, u64, Option<std::time::SystemTime>)>>> = std::sync::OnceLock::new();
    V.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Load model `id` from `dir`: read its card and check its files' sha256 (once per run per file
/// version).
pub fn load(dir: &Path, id: &str) -> Result<InstalledModel> {
    let card = read_card(dir, id)?;
    let mdir = dir.join(id);
    let files: Vec<(PathBuf, &str)> = std::iter::once((mdir.join(&card.file), card.sha256.as_str()))
        .chain(card.extra_files.iter().map(|f| (mdir.join(&f.file), f.sha256.as_str())))
        .collect();
    for (path, want) in files {
        let meta = std::fs::metadata(&path).map_err(|e| MlError::Model(id.into(), format!("{}: {e}", path.display())))?;
        let key = (path.clone(), meta.len(), meta.modified().ok());
        if verified().lock().map(|v| v.contains(&key)).unwrap_or(false) {
            continue;
        }
        if want.is_empty() {
            log::warn!("model `{id}`: no sha256 for {} on its card; not checked", path.display());
        } else {
            let got = sha256_file(&path)?;
            if !got.eq_ignore_ascii_case(want) {
                return Err(MlError::Model(
                    id.into(),
                    format!("{} does not match its card (sha256 {got}, expected {want}): reinstall it", path.display()),
                ));
            }
        }
        if let Ok(mut v) = verified().lock() {
            v.insert(key);
        }
    }
    Ok(InstalledModel { card, dir: mdir })
}

/// Checks one more file of an installed model (a weights variant) against `sha256`, once per run
/// of the app per file version, like [`load`].
pub fn check_file(model: &InstalledModel, file: &str, sha256: &str) -> Result<()> {
    let id = model.card.id.as_str();
    let p = Path::new(file);
    if file.is_empty() || p.is_absolute() || p.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        return Err(MlError::Model(id.into(), format!("`{file}` is not a file in the model folder")));
    }
    let path = model.dir.join(file);
    let meta = std::fs::metadata(&path).map_err(|e| MlError::Model(id.into(), format!("{}: {e}", path.display())))?;
    // Keyed by the expected sha256 too: a card that changes its checksum checks the file again.
    static CHECKED: Mutex<Vec<(PathBuf, u64, Option<std::time::SystemTime>, String)>> = Mutex::new(Vec::new());
    let key = (path.clone(), meta.len(), meta.modified().ok(), sha256.to_ascii_lowercase());
    if CHECKED.lock().map(|v| v.contains(&key)).unwrap_or(false) || sha256.is_empty() {
        return Ok(());
    }
    let got = sha256_file(&path)?;
    if !got.eq_ignore_ascii_case(sha256) {
        return Err(MlError::Model(id.into(), format!("{} does not match its card (sha256 {got}, expected {sha256}): reinstall it", path.display())));
    }
    if let Ok(mut v) = CHECKED.lock() {
        v.push(key);
    }
    Ok(())
}

/// The sha256 of a file, as lowercase hex.
pub fn sha256_file(path: &Path) -> Result<String> {
    use sha2::Digest;
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| MlError::Io(format!("{}: {e}", path.display())))?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).map_err(|e| MlError::Io(format!("{}: {e}", path.display())))?;
        if n == 0 {
            break;
        }
        h.update(buf.get(..n).unwrap_or_default());
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_pre_rename_model_folder_is_used_until_the_new_one_exists() {
        let root = std::env::temp_dir().join(format!("soundcraft-ml-models-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (new, old) = (root.join("FreeDAWm").join("models"), root.join("SoundCraft").join("models"));
        assert_eq!(pick_models_dir(new.clone(), old.clone()), new, "neither: the new folder");
        std::fs::create_dir_all(&old).unwrap();
        assert_eq!(pick_models_dir(new.clone(), old.clone()), old, "only the old one: the old one");
        std::fs::create_dir_all(&new).unwrap();
        assert_eq!(pick_models_dir(new.clone(), old.clone()), new, "both: the new one");
        std::fs::remove_dir_all(root).unwrap();
    }
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sc-ml-models-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn lists_installed_missing_and_broken_models_and_checks_sha256() {
        let dir = tmp("list");
        let m = dir.join("htdemucs");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(m.join("model.onnx"), b"not really onnx").unwrap();
        let sha = sha256_file(&m.join("model.onnx")).unwrap();
        let card = serde_json::json!({"id": "htdemucs", "name": "HTDemucs", "file": "model.onnx", "sha256": sha, "sample_rate": 44100,
            "stems": ["drums", "bass", "other", "vocals"], "license": "MIT", "segment": 7.8, "params": {"overlap": 0.25}});
        std::fs::write(m.join("card.json"), card.to_string()).unwrap();
        std::fs::create_dir_all(dir.join("drumsep_mdx23c")).unwrap();
        std::fs::write(dir.join("drumsep_mdx23c").join("card.json"), r#"{"file": "gone.onnx", "licence": "MIT"}"#).unwrap();

        let l = list(&dir);
        let get = |id: &str| l.iter().find(|e| e.model.id == id).unwrap().clone();
        let ht = get("htdemucs");
        let ModelStatus::Installed(c) = &ht.status else { panic!("{:?}", ht.status) };
        assert_eq!(c.stems.len(), 4);
        assert_eq!(c.num("segment", 0.0), 7.8);
        assert_eq!(c.num("overlap", 0.0), 0.25);
        assert_eq!(ht.size, 15);
        assert!(matches!(get("drumsep_mdx23c").status, ModelStatus::Broken(_)));
        assert_eq!(read_card(&dir, "drumsep_mdx23c").unwrap().license, "MIT");
        assert_eq!(get("srs_small").status, ModelStatus::Missing);
        assert_eq!(get("deepfilternet3").status, ModelStatus::Builtin);

        let ok = load(&dir, "htdemucs").unwrap();
        assert!(ok.onnx().ends_with("model.onnx"));
        assert!(matches!(load(&dir, "karaoke_melroformer"), Err(MlError::NotInstalled(_))));
        // A changed file fails its checksum.
        std::fs::write(m.join("model.onnx"), b"tampered, longer bytes").unwrap();
        assert!(matches!(load(&dir, "htdemucs"), Err(MlError::Model(..))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cards_cannot_name_files_outside_the_model_folder() {
        let dir = tmp("escape");
        std::fs::create_dir_all(dir.join("srs_small")).unwrap();
        std::fs::write(dir.join("srs_small").join("card.json"), r#"{"file": "../../evil.onnx"}"#).unwrap();
        assert!(matches!(read_card(&dir, "srs_small"), Err(MlError::Model(..))));
        assert!(matches!(read_card(&dir, "../x"), Err(MlError::BadInput(_))));
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn licence_status_reads_the_cards_the_exporters_write() {
        // The `licence` notes `tools/ml-export/*/run.py` write into the cards.
        let cases = [
            (
                "Weights: licence unknown (personal use only, owner's decision). Model code (lucidrains BS-RoFormer, ZFTurbo MSS): MIT.",
                LICENCE_NOT_STATED,
            ),
            (
                "Model code (lucidrains BS/Mel-RoFormer, ZFTurbo MSS): MIT. Weights (aufr33, viperx): no licence stated. Private use build only; ask the authors before any redistribution.",
                LICENCE_NOT_STATED,
            ),
            (
                "Training code (ZFTurbo MSS): MIT. Weights: no licence stated by the authors (aufr33, jarredou). Private use build only.",
                LICENCE_NOT_STATED,
            ),
            ("MIT (weights: Kimberley Jensen; model code: lucidrains Mel-RoFormer, ZFTurbo MSS).", "MIT"),
            (
                "Code: MIT (Meta Platforms). Weights: released with the MIT-licensed repo, no separate weights licence stated; one third-party ONNX card claims CC BY-NC 4.0 (unverified). Private use build.",
                "MIT",
            ),
            (
                "ADTOF: CC BY-NC-SA 4.0 (non-commercial, share-alike); the PyTorch port has no licence file. Private use build only.",
                "CC BY-NC-SA 4.0 — non-commercial, personal use",
            ),
            ("Apache-2.0 (code and model, Spotify AB).", "Apache-2.0"),
            (
                "Apache-2.0 (bytedance/piano_transcription README). Weights published on Zenodo by the authors with the code; trained on MAESTRO (CC BY-NC-SA 4.0 dataset).",
                "Apache-2.0",
            ),
            ("MIT (code and weights), Copyright (c) 2025 smulelabs. Full text: LICENSE in the HF repo.", "MIT"),
            ("Submitted to the authors. Ask first", "Submitted to the authors"),
        ];
        for (note, want) in cases {
            assert_eq!(licence_status(note).as_deref(), Some(want), "{note}");
        }
        assert_eq!(licence_status("  "), None);
        // "mit" inside a word is not MIT.
        assert_eq!(licence_status("Permitted for personal use"), Some("Permitted for personal use".into()));
    }

    #[test]
    fn every_model_shows_a_licence_and_the_unlicensed_ones_say_so() {
        for m in KNOWN_MODELS {
            assert!(!m.license.is_empty(), "{}", m.id);
        }
        for id in ["bs_roformer_sw", "karaoke_melroformer", "drumsep_mdx23c"] {
            assert_eq!(known_licence(id), LICENCE_NOT_STATED);
        }
        assert_eq!(known_licence("vocals_melroformer"), "MIT");
        // An installed card's note decides; an empty one falls back to the known status.
        let dir = tmp("licence");
        std::fs::create_dir_all(dir.join("htdemucs")).unwrap();
        std::fs::write(dir.join("htdemucs").join("model.onnx"), b"x").unwrap();
        std::fs::write(dir.join("htdemucs").join("card.json"), r#"{"file": "model.onnx", "licence": "Weights: licence unknown"}"#).unwrap();
        std::fs::create_dir_all(dir.join("srs_small")).unwrap();
        std::fs::write(dir.join("srs_small").join("model.onnx"), b"x").unwrap();
        std::fs::write(dir.join("srs_small").join("card.json"), r#"{"file": "model.onnx"}"#).unwrap();
        let rows = list(&dir);
        let get = |id: &str| rows.iter().find(|m| m.model.id == id).unwrap().licence();
        assert_eq!(get("htdemucs"), LICENCE_NOT_STATED);
        assert_eq!(get("srs_small"), "MIT");
        assert_eq!(get("karaoke_melroformer"), LICENCE_NOT_STATED);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
