//! The plugin browser panel (LUNA-style thumbnail grid): every plugin the app can insert, by
//! type, category and vendor, with favourites, recents and search.

pub mod card;
pub mod catalog;
pub mod drop;
pub mod panel;
pub mod presets;
pub mod scan_job;
pub mod thumbs;

use catalog::BrowserCategory;
use std::collections::BTreeMap;

/// Most recently inserted plugins kept.
pub const RECENT_MAX: usize = 30;

/// The browser's three tabs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BrowserTab {
    #[default]
    Instruments,
    Effects,
    Presets,
}

impl BrowserTab {
    /// The tab's place in the strip (and in [`BrowserState::scroll`]).
    pub fn index(self) -> usize {
        match self {
            BrowserTab::Instruments => 0,
            BrowserTab::Effects => 1,
            BrowserTab::Presets => 2,
        }
    }
}

/// The browser's persisted state (part of `UiState`, so it lands in `ui.json`).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct BrowserState {
    pub tab: BrowserTab,
    pub text: String,
    /// The Presets tab's search.
    pub preset_text: String,
    pub favourites_only: bool,
    pub recent_only: bool,
    pub category: Option<BrowserCategory>,
    pub vendor: Option<String>,
    pub large_cards: bool,
    /// Plugin ids, in the order they were starred.
    pub favourites: Vec<String>,
    /// Plugin ids, newest first, at most [`RECENT_MAX`].
    pub recent: Vec<String>,
    /// Plugin id to the category the user filed it under, over the catalog's own.
    #[serde(deserialize_with = "lenient_overrides")]
    pub category_overrides: BTreeMap<String, BrowserCategory>,
    /// The clicked card, until a module is opened (the Presets tab follows the later of the
    /// two). Not saved (it means nothing after a restart), though `ui.set` may set it.
    #[serde(skip_serializing)]
    pub selected: Option<String>,
    /// The grid's scroll offset per tab (Instruments, Effects, Presets), in points.
    pub scroll: [f32; 3],
}

/// Records `plugin` as just inserted: to the front, once, capped at [`RECENT_MAX`].
pub fn note_inserted(state: &mut BrowserState, plugin: &str) {
    state.recent.retain(|p| p != plugin);
    state.recent.insert(0, plugin.to_string());
    state.recent.truncate(RECENT_MAX);
}

/// The category overrides as saved, leaving out entries that no longer name a category (an older
/// or newer version's), so one of them never resets every preference.
fn lenient_overrides<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<String, BrowserCategory>, D::Error> {
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Entry {
        Known(BrowserCategory),
        Unknown(serde::de::IgnoredAny),
    }
    let raw: BTreeMap<String, Entry> = serde::Deserialize::deserialize(d)?;
    Ok(raw
        .into_iter()
        .filter_map(|(k, v)| match v {
            Entry::Known(c) => Some((k, c)),
            Entry::Unknown(_) => None,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_override_categories_are_skipped_not_fatal() {
        let s: BrowserState =
            serde_json::from_str(r#"{"favourites": ["eq_7band"], "category_overrides": {"a": "Eq", "b": "Spatializer", "c": 5}}"#).unwrap();
        assert_eq!(s.favourites, ["eq_7band"]);
        assert_eq!(s.category_overrides.into_iter().collect::<Vec<_>>(), [("a".to_string(), BrowserCategory::Eq)]);
    }
}
