//! The model catalogue and the downloads manifest: plain data and pure functions (serde only).
//!
//! `cargo xtask ml-publish` includes this file (`#[path]`) so the manifest it writes and the one
//! the app reads are the same types, and the model list is written down once. Keep it free of
//! `crate::` paths and of dependencies other than `serde` and `serde_json`.

use serde::{Deserialize, Serialize};

/// A model the DAW knows how to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownModel {
    pub id: &'static str,
    pub name: &'static str,
    pub job: &'static str,
    /// Built into the app (DeepFilterNet ships inside `deep_filter`): never "missing".
    pub builtin: bool,
    /// The licence status shown before the model is installed (once installed, the card's
    /// `license` field decides: [`licence_status`]). [`LICENCE_NOT_STATED`] when the author states
    /// none.
    pub license: &'static str,
}

/// The licence status shown for a model whose author states no licence.
pub const LICENCE_NOT_STATED: &str = "Licence not stated by the author — personal use";

/// A short licence status from a model card's `license` text (a free-text note such as
/// "Code: MIT. Weights: no licence stated by the authors…"): [`LICENCE_NOT_STATED`] when the
/// weights' licence is unknown or not stated, else the first licence the note names ("MIT",
/// "Apache-2.0", a non-commercial Creative Commons licence), else the note's first sentence.
/// `None` for an empty note.
pub fn licence_status(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let low = text.to_lowercase();
    if ["licence unknown", "license unknown", "no licence stated", "no license stated", "licence not stated", "license not stated"]
        .iter()
        .any(|k| low.contains(k))
    {
        return Some(LICENCE_NOT_STATED.to_string());
    }
    const KNOWN: [(&str, &str); 7] = [
        ("llama 3.2 community license", "Llama 3.2 Community License"),
        ("cc by-nc-sa 4.0", "CC BY-NC-SA 4.0 — non-commercial, personal use"),
        ("cc by-nc 4.0", "CC BY-NC 4.0 — non-commercial, personal use"),
        ("mit or apache-2.0", "MIT OR Apache-2.0"),
        ("apache-2.0", "Apache-2.0"),
        ("apache 2.0", "Apache-2.0"),
        ("mit", "MIT"),
    ];
    let first = KNOWN.iter().filter_map(|(k, label)| find_word(&low, k).map(|at| (at, *label))).min_by_key(|(at, _)| *at);
    if let Some((_, label)) = first {
        return Some(label.to_string());
    }
    Some(text.split(['.', ';', '\n']).next().unwrap_or(text).chars().take(60).collect())
}

/// Where `word` first occurs in `text` on its own (not inside a longer word).
fn find_word(text: &str, word: &str) -> Option<usize> {
    let alnum = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    text.match_indices(word)
        .map(|(at, _)| at)
        .find(|&at| !alnum(text.get(..at).and_then(|b| b.chars().next_back())) && !alnum(text.get(at + word.len()..).and_then(|a| a.chars().next())))
}

/// The licence status known for model `id` before it is installed.
pub fn known_licence(id: &str) -> &'static str {
    KNOWN_MODELS.iter().find(|m| m.id == id).map_or(LICENCE_NOT_STATED, |m| m.license)
}

/// The models the DAW runs, in the order the Models page lists them.
pub const KNOWN_MODELS: &[KnownModel] = &[
    KnownModel { id: "htdemucs", name: "HTDemucs v4 (4 stems)", job: "Separate Stems: vocals, drums, bass, other", builtin: false, license: "MIT" },
    KnownModel {
        id: "bs_roformer_sw",
        name: "BS-RoFormer SW (6 stems)",
        job: "Separate Stems (sw6, best): bass, drums, other, vocals, guitar, piano",
        builtin: false,
        license: LICENCE_NOT_STATED,
    },
    KnownModel {
        id: "vocals_melroformer",
        name: "Mel-Band RoFormer vocals (Kimberley Jensen)",
        job: "Separate Stems (best): vocals; Make Karaoke's vocals",
        builtin: false,
        license: "MIT",
    },
    KnownModel {
        id: "karaoke_melroformer",
        name: "Mel-Band RoFormer Karaoke",
        job: "Make Karaoke: lead vs backing vocals",
        builtin: false,
        license: LICENCE_NOT_STATED,
    },
    KnownModel {
        id: "drumsep_mdx23c",
        name: "MDX23C DrumSep (6 stems)",
        job: "Split Drums: kick, snare, toms, hi-hat, ride, crash",
        builtin: false,
        license: LICENCE_NOT_STATED,
    },
    KnownModel {
        id: "deepfilternet3",
        name: "DeepFilterNet 3",
        job: "Clean Vocal: de-noise (built in)",
        builtin: true,
        license: "MIT OR Apache-2.0",
    },
    KnownModel { id: "srs_small", name: "Smule Renaissance Small", job: "Clean Vocal: de-reverb and cleanup", builtin: false, license: "MIT" },
    KnownModel {
        id: "basicpitch",
        name: "Basic Pitch (Spotify)",
        job: "Convert to MIDI: General (any instrument, pitch bends)",
        builtin: false,
        license: "Apache-2.0",
    },
    KnownModel {
        id: "piano_kong",
        name: "Piano Transcription (Kong et al.)",
        job: "Convert to MIDI: Piano (notes and sustain pedal)",
        builtin: false,
        license: "Apache-2.0",
    },
    KnownModel {
        id: "adtof_drums",
        name: "ADTOF Drums",
        job: "Convert to MIDI: Drums (kick, snare, toms, hi-hat, cymbals)",
        builtin: false,
        license: "CC BY-NC-SA 4.0 — non-commercial, personal use",
    },
    KnownModel {
        id: "game_vocal",
        name: "GAME small (openvpi)",
        job: "Convert to MIDI: Vocal (singing voice, one note at a time)",
        builtin: false,
        license: "MIT",
    },
    KnownModel {
        id: "btc_chords",
        name: "BTC chord recognition (large vocabulary)",
        job: "Detect Chords: chords for the Band's chord lane",
        builtin: false,
        license: "MIT",
    },
    KnownModel {
        id: "whisper_lyrics",
        name: "Whisper large-v3 (whisper.cpp)",
        job: "Transcribe Lyrics: words with times",
        builtin: false,
        license: "MIT",
    },
    KnownModel {
        id: "midi_llm",
        name: "MIDI-LLM (Llama 3.2 1B, 8-bit)",
        job: "Generate MIDI from Text: multitrack General MIDI from a description",
        builtin: false,
        license: "Llama 3.2 Community License",
    },
];

/// Whether a licence status means personal use only (no licence stated, or non-commercial).
pub fn is_personal_use(status: &str) -> bool {
    status.contains("personal use")
}

/// The GitHub repository whose `models-v1` release holds the converted models.
pub const MODELS_REPO: &str = "s0lidr0ck/FreeDAWm";
/// The release tag holding the models (not tied to app versions).
pub const MODELS_TAG: &str = "models-v1";
/// Where the app reads the downloads manifest (`FREEDAWM_MODELS_URL` overrides it).
pub const MANIFEST_URL: &str = "https://github.com/s0lidr0ck/FreeDAWm/releases/download/models-v1/models.json";
/// The largest file a GitHub release asset may be (2 GiB).
pub const ASSET_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
/// The manifest format this build reads and `cargo xtask ml-publish` writes.
pub const MANIFEST_FORMAT: u32 = 1;
/// The largest manifest read (bytes).
pub const MAX_MANIFEST_BYTES: u64 = 1 << 20;

/// `models.json`: what the models release holds.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub format: u32,
    /// The release tag (`models-v1`).
    #[serde(default)]
    pub release: String,
    #[serde(default)]
    pub models: Vec<ManifestModel>,
}

/// One model of the release: a zip of its folder (`card.json`, the ONNX and extra files), or, when
/// `manual`, a model to install by hand (`manual_note` says how).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ManifestModel {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// The card's `exported_utc` (ISO 8601, so newer sorts later).
    #[serde(default)]
    pub version: String,
    /// The zip: absolute, or relative to the manifest's URL. Empty when `manual`.
    #[serde(default)]
    pub url: String,
    /// The zip's size, bytes.
    #[serde(default)]
    pub size: u64,
    /// The zip's sha256, lowercase hex.
    #[serde(default)]
    pub sha256: String,
    /// Short licence status ("MIT", "Licence not stated by the author — personal use"…).
    #[serde(default)]
    pub license: String,
    /// The card's whole licence note.
    #[serde(default)]
    pub license_note: String,
    /// The author's page(s).
    #[serde(default)]
    pub source: String,
    /// No licence stated, or non-commercial: the app asks before downloading.
    #[serde(default)]
    pub personal_use_only: bool,
    /// Not in the release: install by hand.
    #[serde(default)]
    pub manual: bool,
    #[serde(default)]
    pub manual_note: String,
}

impl ManifestModel {
    /// Whether the app can download it.
    pub fn downloadable(&self) -> bool {
        !self.manual && !self.url.is_empty() && self.size > 0 && self.sha256.len() == 64
    }
}

impl Manifest {
    /// Reads `models.json`. Entries with a bad id, or downloadable-looking entries without a
    /// sha256, are refused (the whole manifest: it is ours, so a bad one is a publishing mistake).
    pub fn parse(text: &str) -> Result<Manifest, String> {
        if text.len() as u64 > MAX_MANIFEST_BYTES {
            return Err("models.json is too large".into());
        }
        let m: Manifest = serde_json::from_str(text).map_err(|e| format!("models.json: {e}"))?;
        if m.format > MANIFEST_FORMAT {
            return Err(format!("models.json format {} is newer than this FreeDAWm reads ({MANIFEST_FORMAT}): update FreeDAWm", m.format));
        }
        for e in &m.models {
            if e.id.is_empty() || !e.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                return Err(format!("models.json: bad model id `{}`", e.id));
            }
            if !e.manual && (e.sha256.len() != 64 || !e.sha256.chars().all(|c| c.is_ascii_hexdigit()) || e.size == 0 || e.url.is_empty()) {
                return Err(format!("models.json: `{}` has no url, size or sha256", e.id));
            }
        }
        Ok(m)
    }

    /// The entry for model `id`.
    pub fn get(&self, id: &str) -> Option<&ManifestModel> {
        self.models.iter().find(|m| m.id == id)
    }
}

/// Whether `manifest_version` is newer than an installed card's `installed` version (both the
/// cards' `exported_utc`). An installed model without a version is never offered an update.
pub fn is_newer(manifest_version: &str, installed: &str) -> bool {
    !installed.is_empty() && manifest_version > installed
}

/// A model's URL resolved against the manifest's: absolute stays, relative joins its folder.
pub fn resolve_url(manifest_url: &str, url: &str) -> String {
    if url.contains("://") {
        return url.to_string();
    }
    match manifest_url.rfind('/') {
        Some(at) => format!("{}/{}", manifest_url.get(..at).unwrap_or(""), url.trim_start_matches('/')),
        None => url.to_string(),
    }
}
