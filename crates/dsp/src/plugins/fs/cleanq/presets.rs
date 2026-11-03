//! CleanQ factory presets, embedded from `CleanQ/Presets/Factory/*.json` (copied verbatim into
//! `presets/`), in file-name order.

/// `(name, json)` for every factory preset.
pub static PRESETS: &[(&str, &str)] = &[
    ("Acoustic guitar", include_str!("presets/Acoustic guitar.json")),
    ("Bass guitar", include_str!("presets/Bass guitar.json")),
    ("Drum bus tidy", include_str!("presets/Drum bus tidy.json")),
    ("Electric guitar", include_str!("presets/Electric guitar.json")),
    ("Female vocals", include_str!("presets/Female vocals.json")),
    ("Floor tom", include_str!("presets/Floor tom.json")),
    ("Guitar de-box", include_str!("presets/Guitar de-box.json")),
    ("Hi-hat", include_str!("presets/Hi-hat.json")),
    ("Kick in", include_str!("presets/Kick in.json")),
    ("Kick out", include_str!("presets/Kick out.json")),
    ("Lead guitar", include_str!("presets/Lead guitar.json")),
    ("Male vocals", include_str!("presets/Male vocals.json")),
    ("Mono the lows", include_str!("presets/Mono the lows.json")),
    ("Overheads", include_str!("presets/Overheads.json")),
    ("Piano", include_str!("presets/Piano.json")),
    ("Rack tom", include_str!("presets/Rack tom.json")),
    ("Room mic", include_str!("presets/Room mic.json")),
    ("Snare bottom", include_str!("presets/Snare bottom.json")),
    ("Snare top", include_str!("presets/Snare top.json")),
    ("Spoken word", include_str!("presets/Spoken word.json")),
    ("Strings and pads", include_str!("presets/Strings and pads.json")),
    ("Vocal cleanup", include_str!("presets/Vocal cleanup.json")),
];
