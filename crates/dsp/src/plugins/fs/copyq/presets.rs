//! CopyQ's factory presets, written for FreeDAWm (`presets/`), in file-name order.

/// `(name, json)` for every factory preset.
pub static PRESETS: &[(&str, &str)] = &[
    ("Detailed Match", include_str!("presets/Detailed Match.json")),
    ("Dynamic Match", include_str!("presets/Dynamic Match.json")),
    ("Exaggerate", include_str!("presets/Exaggerate.json")),
    ("Full Match", include_str!("presets/Full Match.json")),
    ("Gentle Match", include_str!("presets/Gentle Match.json")),
    ("Leave The Low End", include_str!("presets/Leave The Low End.json")),
    ("Mastering Reference", include_str!("presets/Mastering Reference.json")),
    ("Mids Only", include_str!("presets/Mids Only.json")),
    ("Push Apart", include_str!("presets/Push Apart.json")),
    ("Stereo Image Match", include_str!("presets/Stereo Image Match.json")),
    ("Tone Only", include_str!("presets/Tone Only.json")),
    ("Tracking (Zero Latency)", include_str!("presets/Tracking (Zero Latency).json")),
];
