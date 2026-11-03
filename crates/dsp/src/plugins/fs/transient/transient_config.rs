//! TransientConfig -- what each Source is for, and what each Focus means, in words.
//! Translation of `Engine 10 - Transient/Source/TransientConfig.h` (the editor's text; nothing
//! here affects the sound). The Source and Focus choices are the same for every source: no
//! control is renamed or hidden per source.

/// A source's name and the line the editor shows under the Source selector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceDef {
    pub name: &'static str,
    pub blurb: &'static str,
}

pub static SOURCES: [SourceDef; 7] = [
    SourceDef { name: "Drums", blurb: "A kit or a single drum. Short, hard attacks; the sustain is the ring and the room." },
    SourceDef { name: "Percussion", blurb: "Hand drums, claps, shakers: faster than a kit, and the detector ignores the lows." },
    SourceDef { name: "Guitar", blurb: "Listens to the pick, 1.2-8 kHz. Put Attack on High for more or less pick, same body." },
    SourceDef { name: "Piano", blurb: "The hammer, then a long ring. Cut Sustain to accent the attack and pull the ring down." },
    SourceDef { name: "Bass", blurb: "Pluck and pick on the low end: slower attacks, a shorter sustain." },
    SourceDef { name: "Vocal", blurb: "Consonants against vowels. Gentle: small ratios count for a lot here." },
    SourceDef { name: "Full Mix", blurb: "Everything at once. Keep the amounts small; it is punch, not surgery." },
];

/// `source (i)`: `jlimit (0, 6, i)`.
pub fn source(i: i32) -> &'static SourceDef {
    SOURCES.get(i.clamp(0, 6) as usize).unwrap_or(&SOURCES[0])
}

/// One line on what the Focus choices do, for under the two selectors.
pub const FOCUS_BLURB: &str =
    "Focus puts each gain on the whole signal, or only the lows (below 220 Hz), mids (around 1 kHz) or highs (above 2.5 kHz).";
