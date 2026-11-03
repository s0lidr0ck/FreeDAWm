//! LevelUp factory presets, embedded from `LevelUp/Presets/Factory/*.json` (copied verbatim into
//! `presets/`), in file-name order.

/// `(name, json)` for every factory preset.
pub static PRESETS: &[(&str, &str)] = &[
    ("A-B In", include_str!("presets/A-B In.json")),
    ("A-B Out", include_str!("presets/A-B Out.json")),
    ("Dialogue Leveler", include_str!("presets/Dialogue Leveler.json")),
    ("Gentle Rider", include_str!("presets/Gentle Rider.json")),
    ("Master Check - CD", include_str!("presets/Master Check - CD.json")),
    ("Master Check - Streaming", include_str!("presets/Master Check - Streaming.json")),
];
