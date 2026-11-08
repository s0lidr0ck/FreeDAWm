//! Juke presets: whole kits to load, each a complete sound: where its samples come from, every
//! pad's tuning, envelopes, filter, drive, level, pan and sends, the kit's levels and its bus
//! effects with their settings (`mix.kit_preset`). The built-in library is
//! `juke_presets.json`; a preset saved from Juke (`mix.kit_preset_save`) is one of these files.
//!
//! A preset starts from its `source` (a starter or style kit by name, a recorded kit by id, or
//! Juke's defaults), takes pads from other recorded kits (`borrow`), then sets what it lists. A saved
//! preset lists everything: every parameter (`params`), each sample (`samples`) and its effects.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use soundcraft_model::DrumFamily;
use std::collections::BTreeMap;

/// One pad taken from a recorded kit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Borrow {
    /// The recorded kit's id (`sm_drums_hybrid`).
    pub kit: String,
    /// Its pad, from 1.
    pub pad: usize,
}

/// The groove a preset is shown off with: a style from `juke_styles` and one of its grooves (its
/// first when `groove` is empty). The preset renders (`tools/kits/`) and any "hear it" button use it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Demo {
    pub style: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub groove: String,
}

/// A preset.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    /// The menu it is listed under (Acoustic, Rock, Hip-Hop…).
    #[serde(default)]
    pub category: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
    /// What it starts from: a starter or style kit's name, a recorded kit's id, or nothing (Juke's
    /// defaults).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// Pads that play another recorded kit's pad: pad (1–16) → that kit and pad.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub borrow: BTreeMap<String, Borrow>,
    /// Pad settings by pad (`"3"`), family (`"hat"`, see [`DrumFamily::id`]) or `"all"`, applied in
    /// that order from `all` to the pad: a parameter without its `pN_` (`tune`, `decay`,
    /// `cutoff`…) and its value, or `name` and `family`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pads: BTreeMap<String, BTreeMap<String, Value>>,
    /// Kit-wide parameters (`level`, `return_a`, `return_b`, `humanize`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub kit: BTreeMap<String, f32>,
    /// The bus effects, replacing the source's (`{bus: a|b|out, slot: 1-4, plugin, params}`);
    /// absent: the source's are kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fx: Option<Vec<Value>>,
    /// The groove it is demonstrated with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub demo: Option<Demo>,
    /// A saved preset: every Juke parameter.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, f32>,
    /// A saved preset: each sample slot (`p1.v1.r1`) → `kit:<recorded kit id>/<file>` (a file of an
    /// installed recorded kit) or `file:<path beside the preset file>`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub samples: BTreeMap<String, String>,
}

impl Preset {
    /// The recorded kits it needs installed: its source (when it is not a starter kit), the kits
    /// it borrows from and those its samples come from.
    pub fn needs(&self) -> Vec<String> {
        let mut v = Vec::new();
        let mut add = |k: &str| {
            if !k.is_empty() && !v.iter().any(|x: &String| x == k) {
                v.push(k.to_string());
            }
        };
        if !self.source.is_empty() && crate::juke_kits::find(&self.source).is_none() {
            add(&self.source);
        }
        for b in self.borrow.values() {
            add(&b.kit);
        }
        for s in self.samples.values() {
            if let Some(rest) = s.strip_prefix("kit:") {
                add(rest.split('/').next().unwrap_or_default());
            }
        }
        v
    }

    /// The settings `pads` gives pad `pad` (from 0) of `family`: `all` first, then the family,
    /// then the pad's own.
    pub fn pad_settings(&self, pad: usize, family: DrumFamily) -> Vec<(&str, &Value)> {
        let mut v = Vec::new();
        for key in ["all".to_string(), family.id().to_string(), (pad + 1).to_string()] {
            if let Some(m) = self.pads.get(&key) {
                v.extend(m.iter().map(|(k, x)| (k.as_str(), x)));
            }
        }
        v
    }
}

/// The built-in presets.
pub fn builtin() -> &'static [Preset] {
    static LIB: std::sync::OnceLock<Vec<Preset>> = std::sync::OnceLock::new();
    LIB.get_or_init(|| serde_json::from_str::<Vec<Preset>>(include_str!("juke_presets.json")).unwrap_or_default())
}

/// The built-in preset named `name` (case and spaces ignored).
pub fn find(name: &str) -> Option<&'static Preset> {
    let key = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let k = key(name);
    builtin().iter().find(|p| key(&p.name) == k)
}

/// The recorded kits' folder, where downloaded and saved kits live (none on the web).
pub fn default_kits_dir() -> Option<std::path::PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Some(soundcraft_ml::kits::kits_dir())
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_reads_and_its_names_are_unique() {
        let lib = builtin();
        assert!(lib.len() >= 30, "{}", lib.len());
        let mut names: Vec<_> = lib.iter().map(|p| p.name.to_lowercase()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), lib.len(), "names are unique");
        for p in lib {
            assert!(!p.category.is_empty(), "{}", p.name);
            let demo = p.demo.as_ref().unwrap_or_else(|| panic!("{}: no demo groove", p.name));
            let style = crate::juke_styles::find(&demo.style).unwrap_or_else(|| panic!("{}: no style {}", p.name, demo.style));
            assert!(demo.groove.is_empty() || style.groove(&demo.groove).is_some(), "{}: no groove {} in {}", p.name, demo.groove, demo.style);
            if !p.source.is_empty() && crate::juke_kits::find(&p.source).is_none() {
                assert!(p.source.starts_with("sm_drums"), "{}: source {}", p.name, p.source);
            }
        }
    }

    #[test]
    fn pad_settings_run_from_all_to_the_pad() {
        let p: Preset = serde_json::from_str(r#"{"name":"x","pads":{"all":{"tune":1},"kick":{"tune":2},"1":{"tune":3}}}"#).unwrap();
        let v = p.pad_settings(0, DrumFamily::Kick);
        assert_eq!(v.iter().map(|(_, x)| x.as_f64().unwrap()).collect::<Vec<_>>(), vec![1.0, 2.0, 3.0]);
        assert_eq!(p.pad_settings(1, DrumFamily::Snare).len(), 1);
        let q: Preset = serde_json::from_str(r#"{"name":"y","source":"sm_drums","borrow":{"3":{"kit":"sm_drums_hybrid","pad":3}}}"#).unwrap();
        assert_eq!(q.needs(), vec!["sm_drums".to_string(), "sm_drums_hybrid".to_string()]);
        let r: Preset = serde_json::from_str(r#"{"name":"z","source":"Porch Light"}"#).unwrap();
        assert!(r.needs().is_empty());
    }
}
