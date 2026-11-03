//! Shale factory presets, embedded from `Shale/Presets/Factory/*.json` (copied verbatim into
//! `presets/`), in file-name order.

/// `(name, json)` for every factory preset.
pub static PRESETS: &[(&str, &str)] = &[
    ("Bleed Only", include_str!("presets/Bleed Only.json")),
    ("Floor Tom - Long", include_str!("presets/Floor Tom - Long.json")),
    ("Gentle Clean-Up", include_str!("presets/Gentle Clean-Up.json")),
    ("Hi-Hat - Snare Out", include_str!("presets/Hi-Hat - Snare Out.json")),
    ("Kick - Duck The Snare", include_str!("presets/Kick - Duck The Snare.json")),
    ("Kick In - Let It Ring", include_str!("presets/Kick In - Let It Ring.json")),
    ("Kick In - Tight", include_str!("presets/Kick In - Tight.json")),
    ("Kick Out - Body", include_str!("presets/Kick Out - Body.json")),
    ("Overheads - Between Fills", include_str!("presets/Overheads - Between Fills.json")),
    ("Rack Tom - Linked Kit", include_str!("presets/Rack Tom - Linked Kit.json")),
    ("Rack Tom - Start Here", include_str!("presets/Rack Tom - Start Here.json")),
    ("Ride - Keep The Wash", include_str!("presets/Ride - Keep The Wash.json")),
    ("Room - Follow The Snare", include_str!("presets/Room - Follow The Snare.json")),
    ("Room - Gated Eighties", include_str!("presets/Room - Gated Eighties.json")),
    ("Snare - Parallel Blend", include_str!("presets/Snare - Parallel Blend.json")),
    ("Snare Bottom - Pair", include_str!("presets/Snare Bottom - Pair.json")),
    ("Snare Top - Hats Be Gone", include_str!("presets/Snare Top - Hats Be Gone.json")),
    ("Snare Top - Keep The Ghosts", include_str!("presets/Snare Top - Keep The Ghosts.json")),
    ("Snare Top - Learned Bleed", include_str!("presets/Snare Top - Learned Bleed.json")),
    ("Snare Top - Start Here", include_str!("presets/Snare Top - Start Here.json")),
    ("Sub Kick", include_str!("presets/Sub Kick.json")),
    ("Toms - Fixed Close", include_str!("presets/Toms - Fixed Close.json")),
    ("Trigger Source", include_str!("presets/Trigger Source.json")),
];
