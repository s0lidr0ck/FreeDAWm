//! Scout sound profiles: a named set of the measurements Scout takes for a strip, kept outside
//! sessions (the library is one JSON file per profile in the settings folder) and used as a
//! strip's target. Loading is lenient like the rest of Scout: a malformed file reads as much as
//! it can, never panics, and caps its sizes.

use super::{ScoutMetrics, lenient_f64, lenient_metrics};
use serde::{Deserialize, Serialize};

/// The roles the panel suggests (a profile's role is free text).
pub const PROFILE_ROLES: [&str; 7] = ["vocal", "drums", "bass", "guitar", "keys", "other", "mix"];

/// Most bytes a profile file may have (a hostile file cannot balloon memory).
pub const MAX_PROFILE_BYTES: usize = 1 << 20;
/// Most tags a profile keeps, and characters per name, role or tag.
pub const MAX_PROFILE_TAGS: usize = 64;
pub const MAX_PROFILE_TEXT: usize = 200;

/// What a profile is of: one sound (a track, a stem) or a whole mix (a mastering target).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    #[default]
    Track,
    Mix,
}

impl ProfileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProfileKind::Track => "track",
            ProfileKind::Mix => "mix",
        }
    }

    pub fn parse(s: &str) -> Option<ProfileKind> {
        match s.trim().to_ascii_lowercase().as_str() {
            "track" => Some(ProfileKind::Track),
            "mix" => Some(ProfileKind::Mix),
            _ => None,
        }
    }
}

/// Where a profile's audio came from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProfileSource {
    /// A file name, a session and track, or a capture ("<session> · <capture> · <strip>").
    #[serde(default)]
    pub file: String,
    /// The measured time range in seconds (0..0: the whole source).
    #[serde(default, deserialize_with = "lenient_f64")]
    pub start_s: f64,
    #[serde(default, deserialize_with = "lenient_f64")]
    pub end_s: f64,
    /// The stem, when the profile was measured on a separated stem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stem: Option<String>,
    /// The separation model that made the stem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// "base" or "post" for a profile taken from a Scout capture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
}

/// One sound profile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScoutProfile {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: ProfileKind,
    /// vocal, drums, bass, guitar, keys, other, mix… (free text).
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub source: ProfileSource,
    /// RFC 3339.
    #[serde(default)]
    pub created: String,
    /// The band design the spectrum was measured with (see `ScoutCapture::bands_version`).
    #[serde(default = "current_bands")]
    pub bands_version: u32,
    #[serde(default, deserialize_with = "lenient_metrics")]
    pub metrics: ScoutMetrics,
}

fn current_bands() -> u32 {
    2
}

fn cap(s: &mut String) {
    if s.chars().count() > MAX_PROFILE_TEXT {
        *s = s.chars().take(MAX_PROFILE_TEXT).collect();
    }
}

impl ScoutProfile {
    /// A profile of `metrics` named `name`, of a track.
    pub fn new(name: &str, metrics: ScoutMetrics) -> ScoutProfile {
        ScoutProfile {
            name: name.to_string(),
            kind: ProfileKind::Track,
            role: String::new(),
            tags: Vec::new(),
            source: ProfileSource::default(),
            created: String::new(),
            bands_version: current_bands(),
            metrics,
        }
    }

    /// Reads a profile file's text leniently: bad numbers keep their defaults, sizes are capped.
    /// Not JSON, too big or without a name: an error.
    pub fn from_json(text: &str) -> Result<ScoutProfile, String> {
        if text.len() > MAX_PROFILE_BYTES {
            return Err("the profile file is too big".into());
        }
        let mut v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("not a profile file: {e}"))?;
        if let Some(o) = v.as_object_mut() {
            // A bad kind or tag list reads as the default rather than failing the whole file.
            if o.get("kind").and_then(|k| k.as_str()).and_then(ProfileKind::parse).is_none() {
                o.remove("kind");
            }
            if let Some(serde_json::Value::Array(tags)) = o.get_mut("tags") {
                tags.retain(serde_json::Value::is_string);
            } else {
                o.remove("tags");
            }
            for k in ["name", "role", "created"] {
                if o.get(k).is_some_and(|x| !x.is_string()) {
                    o.remove(k);
                }
            }
            if o.get("source").is_some_and(|x| !x.is_object()) {
                o.remove("source");
            }
            if o.get("bands_version").is_some_and(|x| x.as_u64().and_then(|n| u32::try_from(n).ok()).is_none()) {
                o.remove("bands_version");
            }
        }
        let mut p: ScoutProfile = serde_json::from_value(v).map_err(|e| format!("not a profile file: {e}"))?;
        p.sanitize();
        if p.name.trim().is_empty() {
            return Err("the profile has no name".into());
        }
        Ok(p)
    }

    /// Caps the texts and the tag list, trims the name.
    pub fn sanitize(&mut self) {
        self.name = self.name.trim().to_string();
        cap(&mut self.name);
        cap(&mut self.role);
        self.tags.truncate(MAX_PROFILE_TAGS);
        self.tags.iter_mut().for_each(cap);
        self.tags.retain(|t| !t.trim().is_empty());
        cap(&mut self.source.file);
    }

    /// The JSON written to a profile file.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Whether the profile matches a search: every word of `query` in its name, role, tags or
    /// source (case-insensitive), and `role` (when not empty) equal to its role.
    pub fn matches(&self, query: &str, role: &str) -> bool {
        if !role.trim().is_empty() && !self.role.trim().eq_ignore_ascii_case(role.trim()) {
            return false;
        }
        let hay = format!("{} {} {} {} {}", self.name, self.role, self.tags.join(" "), self.source.file, self.source.stem.as_deref().unwrap_or(""))
            .to_lowercase();
        query.split_whitespace().all(|w| hay.contains(&w.to_lowercase()))
    }

    /// "<file> · <stem> · 0:12–0:42": where the profile came from, short.
    pub fn source_text(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if !self.source.file.is_empty() {
            parts.push(self.source.file.clone());
        }
        if let Some(stem) = &self.source.stem {
            parts.push(stem.clone());
        }
        if let Some(pos) = &self.source.position {
            parts.push(pos.clone());
        }
        if self.source.end_s > self.source.start_s {
            let t = |s: f64| {
                let s = s.max(0.0).round() as u64;
                format!("{}:{:02}", s / 60, s % 60)
            };
            parts.push(format!("{}–{}", t(self.source.start_s), t(self.source.end_s)));
        }
        parts.join(" · ")
    }
}

/// A file name for a profile name: letters, digits, space, `-`, `_` and `.` kept, the rest `_`,
/// at most 80 characters, never empty or hidden.
pub fn profile_file_stem(name: &str) -> String {
    let s: String = name.trim().chars().map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') { c } else { '_' }).take(80).collect();
    let s = s.trim_matches(['.', ' ']).to_string();
    if s.is_empty() { "profile".into() } else { s }
}
