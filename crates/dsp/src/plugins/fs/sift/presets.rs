//! Sift's factory presets, written for FreeDAWm (`presets/`), in file-name order.

/// `(name, json)` for every factory preset.
pub static PRESETS: &[(&str, &str)] = &[
    ("Close Mic Pops", include_str!("presets/Close Mic Pops.json")),
    ("Firm De-Ess", include_str!("presets/Firm De-Ess.json")),
    ("Gentle Vocal", include_str!("presets/Gentle Vocal.json")),
    ("High Sibilance", include_str!("presets/High Sibilance.json")),
    ("Plosives Only", include_str!("presets/Plosives Only.json")),
    ("Sibilance Only", include_str!("presets/Sibilance Only.json")),
    ("Smooth Wide", include_str!("presets/Smooth Wide.json")),
    ("Spoken Word", include_str!("presets/Spoken Word.json")),
];
