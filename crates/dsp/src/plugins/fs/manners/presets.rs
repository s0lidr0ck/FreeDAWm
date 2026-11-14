//! Manners' factory presets, written for FreeDAWm (`presets/`), in file-name order.

/// `(name, json)` for every factory preset.
pub static PRESETS: &[(&str, &str)] = &[
    ("Bright Mix", include_str!("presets/Bright Mix.json")),
    ("Clear The Mud", include_str!("presets/Clear The Mud.json")),
    ("Drum Bus Clarity", include_str!("presets/Drum Bus Clarity.json")),
    ("Gentle Master", include_str!("presets/Gentle Master.json")),
    ("Open Up", include_str!("presets/Open Up.json")),
    ("Settle Down", include_str!("presets/Settle Down.json")),
    ("Vocal Polish", include_str!("presets/Vocal Polish.json")),
];
