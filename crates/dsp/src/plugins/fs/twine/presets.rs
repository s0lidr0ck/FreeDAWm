//! Twine factory presets, embedded from `Twine/Presets/Factory/*.json` (copied verbatim into
//! `presets/`), in file-name order.

/// `(name, json)` for every factory preset.
pub static PRESETS: &[(&str, &str)] = &[
    ("Bass Tame", include_str!("presets/Bass Tame.json")),
    ("Drum Bus Clip", include_str!("presets/Drum Bus Clip.json")),
    ("Gentle Master", include_str!("presets/Gentle Master.json")),
    ("Kick Heavy Master", include_str!("presets/Kick Heavy Master.json")),
    ("Loud Master", include_str!("presets/Loud Master.json")),
    ("Mix Bus Clip Glue", include_str!("presets/Mix Bus Clip Glue.json")),
    ("Mix Bus Safety", include_str!("presets/Mix Bus Safety.json")),
    ("Multiband Master", include_str!("presets/Multiband Master.json")),
    ("Punchy Master", include_str!("presets/Punchy Master.json")),
    ("Streaming Master", include_str!("presets/Streaming Master.json")),
];
