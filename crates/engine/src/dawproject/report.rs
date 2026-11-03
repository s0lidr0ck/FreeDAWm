//! The import report: what came across from a DAWproject file and what did not, with track names.
//! Shown in a window after the import and saved in the session's Notes tab.

use serde::{Deserialize, Serialize};

/// How an item fared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fate {
    /// Kept as an inactive placeholder slot (a plugin not installed, another DAW's built-in
    /// device): its name and state go back out unchanged on export.
    Placeholder,
    /// Came across, but not exactly (a warped clip at its original speed, a curve straightened).
    Changed,
    /// Not carried over at all.
    Dropped,
}

impl Fate {
    /// "3 placeholders", "1 changed", "2 not carried".
    pub fn counted(self, n: usize) -> String {
        match self {
            Fate::Placeholder if n != 1 => format!("{n} placeholders"),
            _ => format!("{n} {}", self.label().to_lowercase()),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Fate::Placeholder => "Placeholder",
            Fate::Changed => "Changed",
            Fate::Dropped => "Not carried",
        }
    }
}

/// One line of the report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub fate: Fate,
    /// The track it belongs to (empty: the project).
    pub track: String,
    /// What it is ("VST3 Pro-Q 3", "Clip 'Verse'", "Scenes").
    pub what: String,
    /// Why ("not installed", "time-stretched: imported at its original speed").
    pub why: String,
}

/// The whole report.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// The file's name.
    pub file: String,
    /// The application that wrote it ("Studio One 6.5.2").
    pub application: String,
    /// What came across, one summary line each ("12 tracks", "48 audio clips").
    pub carried: Vec<String>,
    /// Plugins loaded with their saved state, as "Track: Plugin".
    pub plugins: Vec<String>,
    /// Everything not carried exactly.
    pub items: Vec<Item>,
}

/// Most report items kept (a hostile file could name millions).
const MAX_ITEMS: usize = 5_000;

impl Report {
    pub fn add(&mut self, fate: Fate, track: &str, what: impl Into<String>, why: impl Into<String>) {
        if self.items.len() < MAX_ITEMS {
            self.items.push(Item { fate, track: track.to_string(), what: what.into(), why: why.into() });
        } else if self.items.len() == MAX_ITEMS {
            self.items.push(Item {
                fate: Fate::Dropped,
                track: String::new(),
                what: "More items".into(),
                why: "the report lists the first 5000".into(),
            });
        }
    }

    /// How many items have `fate`.
    pub fn count(&self, fate: Fate) -> usize {
        self.items.iter().filter(|i| i.fate == fate).count()
    }

    /// The headline: "Imported Song.dawproject from Studio One 6.5: 3 placeholders, 2 not carried".
    pub fn headline(&self) -> String {
        let mut s = format!("Imported {}", self.file);
        if !self.application.is_empty() {
            s.push_str(&format!(" from {}", self.application));
        }
        let parts: Vec<String> = [Fate::Placeholder, Fate::Changed, Fate::Dropped]
            .into_iter()
            .filter_map(|f| {
                let n = self.count(f);
                (n > 0).then(|| f.counted(n))
            })
            .collect();
        if parts.is_empty() {
            s.push_str(": everything came across");
        } else {
            s.push_str(&format!(": {}", parts.join(", ")));
        }
        s
    }

    /// The report as plain text (the note, and `file.import_dawproject`'s result).
    pub fn to_text(&self) -> String {
        let mut out = self.headline();
        out.push('\n');
        if !self.carried.is_empty() {
            out.push_str(&format!("Came across: {}.\n", self.carried.join(", ")));
        }
        if !self.plugins.is_empty() {
            out.push_str(&format!("Plugins loaded with their state: {}.\n", self.plugins.join(", ")));
        }
        for fate in [Fate::Placeholder, Fate::Changed, Fate::Dropped] {
            let items: Vec<&Item> = self.items.iter().filter(|i| i.fate == fate).collect();
            if items.is_empty() {
                continue;
            }
            out.push_str(&format!("{}:\n", fate.label()));
            for i in items {
                let track = if i.track.is_empty() { String::new() } else { format!("{}: ", i.track) };
                out.push_str(&format!("- {track}{} ({})\n", i.what, i.why));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_lists_every_item_by_fate() {
        let mut r = Report { file: "Song.dawproject".into(), application: "Studio One 6.5".into(), ..Report::default() };
        assert!(r.headline().ends_with("everything came across"));
        r.carried.push("2 tracks".into());
        r.add(Fate::Placeholder, "Gtr", "VST3 Amp", "not installed");
        r.add(Fate::Dropped, "", "Scenes", "the clip launcher is not supported");
        let t = r.to_text();
        assert!(t.contains("1 placeholder, 1 not carried"), "{t}");
        assert_eq!(Fate::Placeholder.counted(2), "2 placeholders");
        assert!(t.contains("- Gtr: VST3 Amp (not installed)"), "{t}");
        assert!(t.contains("- Scenes (the clip launcher is not supported)"), "{t}");
        for _ in 0..MAX_ITEMS + 10 {
            r.add(Fate::Dropped, "", "x", "y");
        }
        assert_eq!(r.items.len(), MAX_ITEMS + 1);
    }
}
