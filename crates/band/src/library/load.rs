//! Library loader: Players/*.json and Targets/*.json -> LibraryData. Port of `LibraryLoader.cpp` (the persona
//! prompt, favourites and persona import are not ported).
//! Rules (spec §4): a bad file is skipped whole and named in errors; the first file (by filename)
//! keeps a contested seed, name, slot or target slot; nothing is ever half-loaded.
//!
//! The files come from a [`LibrarySource`]: the embedded factory Library, with the user's
//! `Players/` and `Targets/` files replacing embedded files of the same name.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use super::json::{Var, compare_ignore_case, contains_only_digits, fmt_g, get_int_value, get_trailing_int_value, parse, text_from_bytes};
use super::*;

/// A Library file larger than this is skipped as "too large".
pub const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;

// One row per accepted key; the loader's unknown-key checks read these rows (target data keys) and a LibraryTest
// walk proves the player blocks agree with the parser. block: "player", "player.<block>" or "target.<driver>".
pub static K_KEYS: &[KeyDoc] = &[
    KeyDoc { block: "player", key: "format", type_: "int 1", def: "required", help: "File format version, always 1." },
    KeyDoc { block: "player", key: "name", type_: "string", def: "required", help: "Player name shown in the browser; \"Session\" is reserved." },
    KeyDoc { block: "player", key: "desc", type_: "string", def: "\"\"", help: "One-line description." },
    KeyDoc {
        block: "player",
        key: "seed",
        type_: "int 1..kMaxSeed",
        def: "required",
        help: "Unique id; the first file (by name) keeps a contested seed.",
    },
    KeyDoc {
        block: "player",
        key: "role",
        type_: "enum keys|bass|drums|guitar|ensemble|melodic",
        def: "required",
        help: "Kind of part the player plays.",
    },
    KeyDoc { block: "player", key: "style", type_: "enum (see README)", def: "required", help: "Voicing style." },
    KeyDoc { block: "player", key: "flags", type_: "object", def: "absent", help: "heavy, slap and artic flags." },
    KeyDoc {
        block: "player",
        key: "slots",
        type_: "list of {instrument, slot}",
        def: "required",
        help: "Instruments the player plays; slot is an ordering hint.",
    },
    KeyDoc { block: "player", key: "tags", type_: "list of words", def: "[]", help: "Genre first, then style; stored lower-case." },
    KeyDoc { block: "player", key: "ample", type_: "object", def: "absent", help: "Ample Sound guitar behaviour." },
    KeyDoc { block: "player", key: "fiddle", type_: "object", def: "absent", help: "Indiginus The Fiddle ornaments." },
    KeyDoc { block: "player", key: "bass", type_: "object", def: "absent", help: "MODO BASS 2 playing style." },
    KeyDoc { block: "player", key: "drums", type_: "object", def: "absent", help: "MODO DRUM playing style." },
    KeyDoc { block: "player", key: "violin", type_: "object", def: "absent", help: "Embertone violin bowing." },
    KeyDoc { block: "player", key: "organ", type_: "object", def: "absent", help: "Hammond B-3X playing style." },
    KeyDoc { block: "player", key: "plucked", type_: "object", def: "absent", help: "Plucked Kontakt targets (Evolution, Indiginus)." },
    KeyDoc { block: "player", key: "brass", type_: "object", def: "absent", help: "Blue Street Brass playing style." },
    KeyDoc { block: "player", key: "groove", type_: "object", def: "absent", help: "Where the player sits in time (Natural feel)." },
    KeyDoc { block: "player", key: "feel", type_: "object", def: "required", help: "Timing, velocity, fills and hand ranges." },
    KeyDoc { block: "player", key: "accents", type_: "16 numbers 0.25..2", def: "required", help: "Accent weight per 16th." },
    KeyDoc {
        block: "player",
        key: "accents12",
        type_: "12 numbers 0.25..2",
        def: "ones",
        help: "Accent weight per step of a native 12-step bar (6/8, 3/4).",
    },
    KeyDoc {
        block: "player",
        key: "grids",
        type_: "object E1..E4 > lane0..lane3 > barA|barB > [[step,type,prob,vel,len]]",
        def: "required unless grids12",
        help: "Pattern hits per Energy; every Energy needs a hit.",
    },
    KeyDoc {
        block: "player",
        key: "grids12",
        type_: "object E1..E4 > lane0..lane3 > barA|barB > [[step,type,prob,vel,len]], steps 0..11",
        def: "absent",
        help: "Native 12-step grids for 6/8 and 3/4; a player with grids12 and no grids is 12-only.",
    },
    KeyDoc { block: "player", key: "meters", type_: "list of \"6/8\" | \"3/4\"", def: "both", help: "The meters grids12 serves." },
    KeyDoc {
        block: "player",
        key: "lint",
        type_: "object",
        def: "absent",
        help: "Library lint: { \"allow\": [rule ids this player may break on purpose] }; read by Tools/check-library.js, ignored by the engine.",
    },
    KeyDoc { block: "player.flags", key: "heavy", type_: "bool", def: "false", help: "Heavy part." },
    KeyDoc { block: "player.flags", key: "slap", type_: "bool", def: "false", help: "Slap bass." },
    KeyDoc { block: "player.flags", key: "artic", type_: "int 0..4095", def: "0", help: "Articulation bit mask." },
    KeyDoc { block: "player.feel", key: "swingBase", type_: "float 0..100", def: "required", help: "Swing percent." },
    KeyDoc { block: "player.feel", key: "swingSteps", type_: "int 1..16", def: "required", help: "Swing grid in 16ths." },
    KeyDoc { block: "player.feel", key: "timingMs", type_: "float -100..100", def: "required", help: "Overall timing offset in ms." },
    KeyDoc { block: "player.feel", key: "rollMs", type_: "float 0..200", def: "required", help: "Chord roll spread in ms." },
    KeyDoc { block: "player.feel", key: "jitterMs", type_: "float 0..100", def: "required", help: "Random timing jitter in ms." },
    KeyDoc { block: "player.feel", key: "velVar", type_: "float 0..64", def: "required", help: "Random velocity variation." },
    KeyDoc { block: "player.feel", key: "arc", type_: "float 0..1", def: "required", help: "Velocity arc over the phrase." },
    KeyDoc { block: "player.feel", key: "pedalMode", type_: "int 0..6", def: "required", help: "Pedal behaviour." },
    KeyDoc { block: "player.feel", key: "fillRate", type_: "float 0..1", def: "required", help: "How often fills happen." },
    KeyDoc { block: "player.feel", key: "fillCount", type_: "int 0..16", def: "required", help: "Notes in a fill." },
    KeyDoc { block: "player.feel", key: "rhLow", type_: "int 0..127", def: "required", help: "Right hand lowest note." },
    KeyDoc { block: "player.feel", key: "rhHigh", type_: "int 0..127", def: "required", help: "Right hand highest note." },
    KeyDoc { block: "player.feel", key: "rhAnchor", type_: "int 0..127", def: "required", help: "Right hand anchor note." },
    KeyDoc { block: "player.feel", key: "lhLow", type_: "int 0..127", def: "required", help: "Left hand lowest note." },
    KeyDoc { block: "player.feel", key: "laneMs", type_: "4 numbers", def: "required", help: "Timing offset per drum lane in ms." },
    KeyDoc { block: "player.groove", key: "timing", type_: "16 numbers -30..30", def: "zeros", help: "Ms offset per 16th of the bar." },
    KeyDoc {
        block: "player.groove",
        key: "timing12",
        type_: "12 numbers -30..30",
        def: "zeros",
        help: "Ms offset per step of a native 12-step bar.",
    },
    KeyDoc { block: "player.groove", key: "push", type_: "float 0..1", def: "0", help: "Anticipation." },
    KeyDoc { block: "player.groove", key: "pickups", type_: "float 0..1", def: "0", help: "Section lead-ins." },
    KeyDoc { block: "player.groove", key: "variation", type_: "float 0..1", def: "0", help: "Phrase variation." },
    KeyDoc { block: "player.groove", key: "space", type_: "float 0..1", def: "0", help: "Air between phrases." },
    KeyDoc {
        block: "player.ample",
        key: "mode",
        type_: "enum strum|notes|auto",
        def: "by role",
        help: "Strummer, single notes, or decided per Energy.",
    },
    KeyDoc { block: "player.ample", key: "openStringsFirst", type_: "bool", def: "false", help: "Prefer open strings." },
    KeyDoc { block: "player.ample", key: "positionMode", type_: "enum chord|melody", def: "chord", help: "Fret position style." },
    KeyDoc { block: "player.ample", key: "palmMute", type_: "float 0..1", def: "0", help: "Palm mute amount." },
    KeyDoc { block: "player.ample", key: "pops", type_: "float 0..1", def: "0", help: "Pop amount." },
    KeyDoc { block: "player.ample", key: "harmonics", type_: "float 0..1", def: "0", help: "Harmonics amount." },
    KeyDoc { block: "player.ample", key: "hammerOn", type_: "float 0..1", def: "0", help: "Hammer-on amount." },
    KeyDoc { block: "player.ample", key: "slides", type_: "float 0..1", def: "0", help: "Slide amount." },
    KeyDoc { block: "player.ample", key: "slideIn", type_: "float 0..1", def: "0", help: "Slide-in amount." },
    KeyDoc { block: "player.ample", key: "fx", type_: "float 0..1", def: "0", help: "FX noise amount." },
    KeyDoc { block: "player.ample", key: "muted", type_: "float 0..1", def: "0", help: "Muted strums." },
    KeyDoc { block: "player.ample", key: "bassStrings", type_: "bool", def: "true", help: "Strum includes the bass strings." },
    KeyDoc { block: "player.ample", key: "strumNoise", type_: "float 0..1", def: "0", help: "Strum noise." },
    KeyDoc { block: "player.ample", key: "capo", type_: "int 0..7", def: "0", help: "Chord shapes fingered from a capo on this fret." },
    KeyDoc {
        block: "player.ample",
        key: "strumInversion",
        type_: "int 0..2",
        def: "0",
        help: "Chord inversion for strums (0 root, 1 third, 2 fifth lowest).",
    },
    KeyDoc { block: "player.fiddle", key: "scoops", type_: "float 0..1", def: "0", help: "Scoops." },
    KeyDoc { block: "player.fiddle", key: "graces", type_: "float 0..1", def: "0", help: "Grace notes." },
    KeyDoc { block: "player.fiddle", key: "graceSlides", type_: "float 0..1", def: "0", help: "Grace slides." },
    KeyDoc { block: "player.fiddle", key: "trills", type_: "float 0..1", def: "0", help: "Trills." },
    KeyDoc { block: "player.fiddle", key: "slides", type_: "float 0..1", def: "0", help: "Slides." },
    KeyDoc { block: "player.fiddle", key: "doubleStops", type_: "float 0..1", def: "0", help: "Double stops." },
    KeyDoc { block: "player.fiddle", key: "shuffle", type_: "float 0..1", def: "0", help: "Shuffle bowing." },
    KeyDoc { block: "player.fiddle", key: "chop", type_: "float 0..1", def: "0", help: "Chops." },
    KeyDoc { block: "player.fiddle", key: "kickoff", type_: "float 0..1", def: "0", help: "Kickoffs." },
    KeyDoc { block: "player.fiddle", key: "fillLicks", type_: "float 0..1", def: "0", help: "Fill licks." },
    KeyDoc { block: "player.fiddle", key: "turnaround", type_: "float 0..1", def: "0", help: "Turnarounds." },
    KeyDoc { block: "player.fiddle", key: "tag", type_: "float 0..1", def: "0", help: "Tags." },
    KeyDoc { block: "player.fiddle", key: "shuffleStyle", type_: "enum nashville|georgia|longbow", def: "nashville", help: "Shuffle flavour." },
    KeyDoc { block: "player.bass", key: "vibrato", type_: "float 0..1", def: "0", help: "Vibrato." },
    KeyDoc { block: "player.bass", key: "letRing", type_: "float 0..1", def: "0", help: "Let ring." },
    KeyDoc { block: "player.bass", key: "falls", type_: "float 0..1", def: "0", help: "Falls." },
    KeyDoc { block: "player.bass", key: "position", type_: "enum low|mid|high", def: "low", help: "Left-hand position." },
    KeyDoc { block: "player.bass", key: "pluck", type_: "enum neck|mid|bridge", def: "mid", help: "Pluck position." },
    KeyDoc { block: "player.drums", key: "crossStick", type_: "float 0..1", def: "0", help: "Cross-stick." },
    KeyDoc { block: "player.drums", key: "pedalHat", type_: "float 0..1", def: "0", help: "Pedal hat." },
    KeyDoc { block: "player.drums", key: "flams", type_: "float 0..1", def: "0", help: "Flams." },
    KeyDoc { block: "player.drums", key: "hatBark", type_: "float 0..1", def: "0", help: "Hat bark." },
    KeyDoc { block: "player.drums", key: "crashAlternate", type_: "float 0..1", def: "0", help: "Alternate crashes." },
    KeyDoc { block: "player.drums", key: "aperture", type_: "float 0..1", def: "0", help: "Hat aperture." },
    KeyDoc { block: "player.drums", key: "china", type_: "float 0..1", def: "0", help: "China cymbal." },
    KeyDoc { block: "player.drums", key: "rideSwitch", type_: "float 0..1", def: "0", help: "Ride switch." },
    KeyDoc { block: "player.drums", key: "bellCymbal", type_: "float 0..1", def: "0", help: "Bell cymbal." },
    KeyDoc { block: "player.violin", key: "portamento", type_: "float 0..1", def: "0", help: "Portamento." },
    KeyDoc { block: "player.violin", key: "doubleStops", type_: "float 0..1", def: "0", help: "Double stops." },
    KeyDoc { block: "player.violin", key: "vibratoDepth", type_: "float 0..1", def: "0.7", help: "Vibrato depth." },
    KeyDoc { block: "player.violin", key: "rebow", type_: "float 0..1", def: "0.7", help: "Rebow." },
    KeyDoc { block: "player.organ", key: "percussion", type_: "float 0..1", def: "0", help: "Percussion." },
    KeyDoc { block: "player.organ", key: "swell", type_: "float 0..1", def: "0", help: "Swell pedal." },
    KeyDoc { block: "player.organ", key: "leslieFast", type_: "float 0..1", def: "0", help: "Leslie fast." },
    KeyDoc { block: "player.organ", key: "build", type_: "float 0..1", def: "0", help: "Drawbar build." },
    KeyDoc { block: "player.organ", key: "leslieKick", type_: "float 0..1", def: "0", help: "Leslie kick." },
    KeyDoc { block: "player.organ", key: "smear", type_: "float 0..1", def: "0", help: "Glissando smear." },
    KeyDoc { block: "player.organ", key: "lowerHand", type_: "float 0..1", def: "0", help: "Lower hand." },
    KeyDoc { block: "player.organ", key: "chorus", type_: "enum off|V1|V2|V3|C1|C2|C3", def: "off", help: "Chorus / vibrato." },
    KeyDoc { block: "player.organ", key: "registration", type_: "enum gospel|jazz|rock|pad", def: "gospel", help: "Drawbar registration." },
    KeyDoc { block: "player.plucked", key: "mode", type_: "enum auto|strum|notes", def: "auto", help: "Strum or notes." },
    KeyDoc { block: "player.plucked", key: "strumMs", type_: "int 0..100", def: "18", help: "Strum spread in ms." },
    KeyDoc { block: "player.plucked", key: "upWeight", type_: "float 0..1", def: "0.75", help: "Up-stroke weight." },
    KeyDoc { block: "player.plucked", key: "upStrings", type_: "int 1..6", def: "3", help: "Strings on an up-stroke." },
    KeyDoc {
        block: "player.plucked",
        key: "rolls",
        type_: "int 0..4",
        def: "0",
        help: "Roll pattern (0 none, 1 forward, 2 backward, 3 alternating, 4 foggy).",
    },
    KeyDoc { block: "player.plucked", key: "chop", type_: "float 0..1", def: "0", help: "Chop." },
    KeyDoc { block: "player.plucked", key: "tremolo", type_: "float 0..1", def: "0", help: "Tremolo." },
    KeyDoc { block: "player.plucked", key: "clawhammer", type_: "float 0..1", def: "0", help: "Clawhammer." },
    KeyDoc { block: "player.plucked", key: "dropThumb", type_: "float 0..1", def: "0", help: "Drop thumb." },
    KeyDoc { block: "player.plucked", key: "hammer", type_: "float 0..1", def: "0", help: "Hammer-ons." },
    KeyDoc { block: "player.plucked", key: "slides", type_: "float 0..1", def: "0", help: "Slides." },
    KeyDoc { block: "player.plucked", key: "graces", type_: "float 0..1", def: "0", help: "Grace notes." },
    KeyDoc { block: "player.plucked", key: "buzz", type_: "float 0..1", def: "0", help: "Buzz." },
    KeyDoc { block: "player.plucked", key: "palmMute", type_: "float 0..1", def: "0", help: "Palm mute." },
    KeyDoc { block: "player.plucked", key: "harmonics", type_: "float 0..1", def: "0", help: "Harmonics." },
    KeyDoc { block: "player.plucked", key: "vibrato", type_: "float 0..1", def: "0.5", help: "Vibrato." },
    KeyDoc { block: "player.plucked", key: "kickoff", type_: "float 0..1", def: "0", help: "Kickoffs." },
    KeyDoc { block: "player.plucked", key: "fillLicks", type_: "float 0..1", def: "0", help: "Fill licks." },
    KeyDoc { block: "player.plucked", key: "turnaround", type_: "float 0..1", def: "0", help: "Turnarounds." },
    KeyDoc { block: "player.brass", key: "stab", type_: "float 0..1", def: "0", help: "Stabs." },
    KeyDoc { block: "player.brass", key: "swell", type_: "float 0..1", def: "0", help: "Swells." },
    KeyDoc { block: "player.brass", key: "voiced", type_: "float 0..1", def: "0", help: "Voiced chords." },
    KeyDoc { block: "player.brass", key: "fall", type_: "float 0..1", def: "0", help: "Falls." },
    KeyDoc { block: "player.brass", key: "legato", type_: "float 0..1", def: "0", help: "Legato." },
    KeyDoc { block: "player.brass", key: "double", type_: "enum none|octave|thirds", def: "none", help: "Doubling." },
    KeyDoc {
        block: "target.drumMap",
        key: "rules",
        type_: "list of {note,minVel,maxVel,side,out,offset}",
        def: "required",
        help: "Note mapping rules, first match wins.",
    },
    KeyDoc { block: "target.drumMap", key: "chokes", type_: "object note > note", def: "{}", help: "Cymbal choke notes." },
    KeyDoc { block: "target.drumMap", key: "hatShank", type_: "2 notes", def: "[-1,-1]", help: "Hat shank closed, open." },
    KeyDoc { block: "target.drumMap", key: "hatExtraOpen", type_: "int 0..127", def: "-1", help: "Extra-open hat note." },
    KeyDoc { block: "target.drumMap", key: "hatFootSplash", type_: "int 0..127", def: "-1", help: "Foot splash note." },
    KeyDoc { block: "target.drumMap", key: "rideBell", type_: "int 0..127", def: "-1", help: "Ride bell note." },
    KeyDoc { block: "target.drumMap", key: "crossStick", type_: "int 0..127", def: "-1", help: "Cross-stick note." },
    KeyDoc { block: "target.drumMap", key: "pedalHat", type_: "int 0..127", def: "-1", help: "Pedal hat note." },
    KeyDoc { block: "target.drumMap", key: "openHatShank", type_: "int 0..127", def: "-1", help: "Open hat shank note." },
    KeyDoc { block: "target.drumMap", key: "splash", type_: "int 0..127", def: "-1", help: "Splash note." },
    KeyDoc { block: "target.drumMap", key: "crashes", type_: "2 notes", def: "[-1,-1]", help: "Two crash notes." },
    KeyDoc { block: "target.drumMap", key: "hatApertureCC", type_: "int -1..127", def: "-1", help: "Hat aperture CC." },
    KeyDoc { block: "target.drumMap", key: "hatCCKeys", type_: "object {shank,tip}", def: "absent", help: "Notes for the CC-following hat." },
    KeyDoc { block: "target.drumMap", key: "china", type_: "int 0..127", def: "-1", help: "China note." },
    KeyDoc { block: "target.drumMap", key: "ride2", type_: "int 0..127", def: "-1", help: "Ride 2 note." },
    KeyDoc { block: "target.drumMap", key: "bellCymbal", type_: "int -1..127", def: "-1", help: "Bell cymbal note." },
    KeyDoc { block: "target.keyswitchBass", key: "finger", type_: "int 0..127", def: "25", help: "Finger keyswitch." },
    KeyDoc { block: "target.keyswitchBass", key: "pick", type_: "int 0..127", def: "27", help: "Pick keyswitch." },
    KeyDoc { block: "target.keyswitchBass", key: "slap", type_: "int 0..127", def: "30", help: "Slap keyswitch." },
    KeyDoc { block: "target.keyswitchBass", key: "ghost", type_: "int 0..127", def: "22", help: "Ghost keyswitch." },
    KeyDoc { block: "target.keyswitchBass", key: "palmMuteCC", type_: "int 0..127", def: "9", help: "Palm mute CC." },
    KeyDoc { block: "target.keyswitchBass", key: "palmMuteOn", type_: "int 0..127", def: "96", help: "Palm mute on value." },
    KeyDoc { block: "target.keyswitchBass", key: "writtenPitchShift", type_: "int -48..48", def: "12", help: "Written pitch shift." },
    KeyDoc { block: "target.keyswitchBass", key: "minNote", type_: "int 0..127", def: "40", help: "Lowest note." },
    KeyDoc { block: "target.keyswitchBass", key: "hammerKS", type_: "int 0..127", def: "-1", help: "Hammer-on keyswitch." },
    KeyDoc { block: "target.keyswitchBass", key: "stopKS", type_: "int 0..127", def: "-1", help: "Stop-on-detach keyswitch." },
    KeyDoc { block: "target.keyswitchBass", key: "downStrokeKS", type_: "int 0..127", def: "-1", help: "Down-stroke keyswitch." },
    KeyDoc { block: "target.keyswitchBass", key: "legatoCC", type_: "int 0..119", def: "-1", help: "Legato slide CC." },
    KeyDoc { block: "target.keyswitchBass", key: "vibratoCC", type_: "int 0..119", def: "-1", help: "Vibrato CC." },
    KeyDoc { block: "target.keyswitchBass", key: "letRingCC", type_: "int 0..119", def: "-1", help: "Let ring CC." },
    KeyDoc { block: "target.keyswitchBass", key: "pluckCC", type_: "int -1..119", def: "-1", help: "Pluck position CC." },
    KeyDoc { block: "target.keyswitchBass", key: "bendRange", type_: "int 1..24", def: "4", help: "Bend range in semitones." },
    KeyDoc { block: "target.keyswitchBass", key: "positionKeys", type_: "2 notes", def: "[-1,-1]", help: "Left-hand position keys lo, hi." },
    KeyDoc {
        block: "target.keyswitchBass",
        key: "positionCC",
        type_: "int -1..119",
        def: "-1",
        help: "Left-hand position CC: fret 0-24 spread over 0-127 (MODO: 4).",
    },
    KeyDoc { block: "target.keyswitchBass", key: "muteRange", type_: "2 notes", def: "[-1,-1]", help: "CC9 range lo, hi." },
    KeyDoc { block: "target.pianoPedal", key: "halfPedal", type_: "3 numbers 0..127", def: "[24,70,127]", help: "Half-pedal levels." },
    KeyDoc { block: "target.pianoPedal", key: "softPedalCC", type_: "int 0..127", def: "67", help: "Soft pedal CC." },
    KeyDoc { block: "target.pianoPedal", key: "softPedalMaxEnergy", type_: "int 0..4", def: "1", help: "Highest Energy using the soft pedal." },
    KeyDoc { block: "target.pianoPedal", key: "harmonicPedalCC", type_: "int -1..119", def: "-1", help: "Harmonic pedal CC." },
    KeyDoc { block: "target.swam", key: "family", type_: "enum strings|brass|winds", def: "required", help: "Instrument family." },
    KeyDoc { block: "target.swam", key: "instruments", type_: "list of {name,low,high}", def: "required", help: "Instruments and ranges." },
    KeyDoc { block: "target.swam", key: "expressionCC", type_: "int -1..127", def: "11", help: "Expression CC." },
    KeyDoc { block: "target.swam", key: "vibratoCC", type_: "int -1..127", def: "1", help: "Vibrato CC." },
    KeyDoc { block: "target.swam", key: "keyswitchOffset", type_: "int 0..127", def: "24", help: "Keyswitch offset." },
    KeyDoc { block: "target.swam", key: "keyswitches", type_: "bool", def: "true", help: "Send SWAM's own keyswitches." },
    KeyDoc { block: "target.swam", key: "dynamics", type_: "enum cc|velocity", def: "cc", help: "Dynamics carrier." },
    KeyDoc { block: "target.swam", key: "velocityRange", type_: "2 numbers", def: "[1,127]", help: "Velocity range for velocity dynamics." },
    KeyDoc {
        block: "target.swam",
        key: "articulations",
        type_: "object name > {key,mode}",
        def: "absent",
        help: "Custom articulation keys (sustain, staccato, pizzicato, tremolo, harmonic).",
    },
    KeyDoc { block: "target.articulationKeys", key: "keys", type_: "object {long,short,pluck,tremolo}", def: "required", help: "Articulation keys." },
    KeyDoc { block: "target.articulationKeys", key: "keyBase", type_: "int 0..127", def: "0", help: "Key base." },
    KeyDoc { block: "target.articulationKeys", key: "customKeys", type_: "bool", def: "false", help: "Use the custom keys." },
    KeyDoc { block: "target.articulationKeys", key: "dynamicsCC", type_: "int -1..127", def: "-1", help: "Dynamics CC." },
    KeyDoc { block: "target.articulationKeys", key: "expressionCC", type_: "int -1..127", def: "11", help: "Expression CC." },
    KeyDoc { block: "target.articulationKeys", key: "tremolo", type_: "bool", def: "false", help: "Library has a tremolo." },
    KeyDoc { block: "target.articulationKeys", key: "pluckCapBelowEnergy", type_: "int 0..5", def: "0", help: "Cap plucks below this Energy." },
    KeyDoc { block: "target.articulationKeys", key: "pluckVelCap", type_: "int 1..127", def: "127", help: "Pluck velocity cap." },
    KeyDoc {
        block: "target.keyswitch",
        key: "articulations",
        type_: "object name > {key,mode,minLen}",
        def: "required",
        help: "Articulation keys (at most 16); mode tap|hold|latch|same, minLen ms (hold).",
    },
    KeyDoc {
        block: "target.keyswitch",
        key: "use",
        type_: "object intent > articulation",
        def: "{}",
        help: "Which articulation plays each intent (long, short, pluck, tremolo, legatoUp, slideIn, ghost).",
    },
    KeyDoc { block: "target.keyswitch", key: "keyBase", type_: "int 0..127", def: "0", help: "Added to every articulation key." },
    KeyDoc { block: "target.keyswitch", key: "dynamicsCC", type_: "int -1..127", def: "-1", help: "Dynamics CC." },
    KeyDoc { block: "target.keyswitch", key: "expressionCC", type_: "int -1..127", def: "-1", help: "Expression CC." },
    KeyDoc { block: "target.keyswitch", key: "reset", type_: "object {dynamics,expression}", def: "absent", help: "CC values sent on stop." },
    KeyDoc { block: "target.keyswitch", key: "sendOnChangeOnly", type_: "bool", def: "true", help: "Tap an articulation only when it changes." },
    KeyDoc { block: "target.keyswitch", key: "pluckCapBelowEnergy", type_: "int 0..5", def: "0", help: "Cap pluck velocity below this Energy." },
    KeyDoc { block: "target.keyswitch", key: "pluckVelCap", type_: "int 1..127", def: "127", help: "Pluck velocity cap." },
    KeyDoc { block: "target.keyswitch", key: "default", type_: "articulation name", def: "none", help: "The articulation when no intent is mapped." },
    KeyDoc {
        block: "target.keyswitch",
        key: "ccLeadMs",
        type_: "object {dynamics,expression} ms 0..100",
        def: "{15,10}",
        help: "CC lead ahead of the step.",
    },
    KeyDoc {
        block: "target.keyswitch",
        key: "keyLeadMs",
        type_: "float 0..40",
        def: "10",
        help: "Hold / latch / same keys this far ahead of their note.",
    },
    KeyDoc { block: "target.keyswitch", key: "return", type_: "enum none|same|key", def: "none", help: "Latch return before the next articulation." },
    KeyDoc { block: "target.keyswitch", key: "returnKey", type_: "int -1..127", def: "-1", help: "Return key (return: key)." },
    KeyDoc { block: "target.keyswitch", key: "low", type_: "int 0..127", def: "0", help: "Lowest playable note (fold by octave)." },
    KeyDoc { block: "target.keyswitch", key: "high", type_: "int 0..127", def: "127", help: "Highest playable note (fold by octave)." },
    KeyDoc { block: "target.keyswitch", key: "velocityRange", type_: "2 numbers 1..127", def: "[1,127]", help: "Velocity clamp." },
    KeyDoc { block: "target.keyswitch", key: "vibratoCC", type_: "int -1..127", def: "-1", help: "Vibrato CC over notes of a beat or more." },
    KeyDoc { block: "target.keyswitch", key: "vibratoDepth", type_: "float 0..1", def: "0.5", help: "Vibrato depth." },
    KeyDoc { block: "target.keyswitch", key: "init", type_: "up to 8 keys 0..127", def: "[]", help: "Setup keys tapped once per pass." },
    KeyDoc { block: "target.keyswitch", key: "never", type_: "up to 8 keys 0..127", def: "[]", help: "Keys never sent (kept takes too)." },
    KeyDoc { block: "target.ujamPhrase", key: "phraseKeys", type_: "list of numbers 0..127", def: "defaults", help: "Style phrase keys." },
    KeyDoc { block: "target.ujamPhrase", key: "additions", type_: "4 numbers -1..127", def: "[25,27,30,32]", help: "Phrase-end additions." },
    KeyDoc { block: "target.ujamPhrase", key: "endingKeys", type_: "1..5 numbers -1..127", def: "defaults", help: "Ending keys." },
    KeyDoc { block: "target.ujamPhrase", key: "bassMode", type_: "enum always|slash|none", def: "always", help: "Bass note mode." },
    KeyDoc { block: "target.ujamPhrase", key: "energyBase", type_: "4 numbers 0..5", def: "[0,2,4,5]", help: "Phrase base per Energy." },
    KeyDoc { block: "target.ujamPhrase", key: "stopKey", type_: "int -1..127", def: "35", help: "Stop key." },
    KeyDoc { block: "target.ujamPhrase", key: "playBase", type_: "int 0..127", def: "60", help: "Play range low." },
    KeyDoc { block: "target.ujamPhrase", key: "playHigh", type_: "int 0..127", def: "83", help: "Play range high." },
    KeyDoc { block: "target.ujamPhrase", key: "bassBase", type_: "int 0..127", def: "48", help: "Bass range low." },
    KeyDoc { block: "target.ujamPhrase", key: "modCC", type_: "int -1..127", def: "1", help: "Mod wheel CC." },
    KeyDoc { block: "target.ampleGuitar", key: "noteRange", type_: "2 notes", def: "required", help: "Playable range." },
    KeyDoc { block: "target.ampleGuitar", key: "noteVelocity", type_: "2 numbers 1..126", def: "required", help: "Note velocity range." },
    KeyDoc {
        block: "target.ampleGuitar",
        key: "articulations",
        type_: "object",
        def: "required",
        help: "sustain, harmonic, palmMute, slideIn, legatoSlide, hammerOn keys.",
    },
    KeyDoc { block: "target.ampleGuitar", key: "openStrings", type_: "list of notes", def: "required", help: "Open string notes." },
    KeyDoc {
        block: "target.ampleGuitar",
        key: "modeKeys",
        type_: "object",
        def: "required",
        help: "strumMode, chordDetect, positionMode, openStringsFirst keys.",
    },
    KeyDoc { block: "target.ampleGuitar", key: "onVelocity", type_: "int 64..127", def: "100", help: "Key-on velocity." },
    KeyDoc { block: "target.ampleGuitar", key: "offVelocity", type_: "int 1..63", def: "20", help: "Key-off velocity." },
    KeyDoc { block: "target.ampleGuitar", key: "chordRange", type_: "2 notes", def: "required", help: "Chord detect range." },
    KeyDoc { block: "target.ampleGuitar", key: "rootHigh", type_: "int 0..127", def: "required", help: "Highest root." },
    KeyDoc { block: "target.ampleGuitar", key: "strings", type_: "list of notes", def: "required", help: "String keys." },
    KeyDoc {
        block: "target.ampleGuitar",
        key: "strokes",
        type_: "object",
        def: "required",
        help: "down, up, downMuted, upMuted key pairs [full, treble].",
    },
    KeyDoc { block: "target.ampleGuitar", key: "strokeVelocity", type_: "2 numbers", def: "required", help: "Stroke velocity range." },
    KeyDoc { block: "target.ampleGuitar", key: "strumTimeKey", type_: "int -1..127", def: "-1", help: "Strum time key." },
    KeyDoc { block: "target.ampleGuitar", key: "strumTimeVelocity", type_: "2 numbers", def: "[118,126]", help: "Strum time velocity range." },
    KeyDoc { block: "target.ampleGuitar", key: "releaseVelocity", type_: "bool", def: "false", help: "Send release velocity." },
    KeyDoc { block: "target.ampleGuitar", key: "onsetMs", type_: "int 0..120", def: "0", help: "Onset delay in ms." },
    KeyDoc { block: "target.ampleGuitar", key: "muteVoices", type_: "list of notes", def: "required", help: "Mute voice keys." },
    KeyDoc { block: "target.ampleGuitar", key: "strokeNoise", type_: "object", def: "required", help: "down, up note pairs." },
    KeyDoc {
        block: "target.ampleGuitar",
        key: "fx",
        type_: "object",
        def: "required",
        help: "hitTop, hitTopMuted, strumMute keys (slap, scratch optional).",
    },
    KeyDoc { block: "target.fiddle", key: "range", type_: "2 notes", def: "[55,91]", help: "Playable range." },
    KeyDoc { block: "target.fiddle", key: "expressionCC", type_: "int -1..127", def: "11", help: "Expression CC." },
    KeyDoc { block: "target.fiddle", key: "splits", type_: "object", def: "absent", help: "vibrato, portamento, slur split velocities." },
    KeyDoc { block: "target.fiddle", key: "articulations", type_: "object", def: "absent", help: "Articulation keys." },
    KeyDoc { block: "target.fiddle", key: "portamentoKey", type_: "int -1..127", def: "50", help: "Portamento key." },
    KeyDoc { block: "target.fiddle", key: "articulationReturn", type_: "object", def: "absent", help: "Return mode: unconfirmed, same, key, hold." },
    KeyDoc { block: "target.fiddle", key: "bowSeconds", type_: "float 0.5..20", def: "3", help: "Seconds per bow." },
    KeyDoc { block: "target.organ", key: "expressionCC", type_: "int -1..119", def: "11", help: "Swell CC." },
    KeyDoc { block: "target.organ", key: "leslieSpeedCC", type_: "int -1..119", def: "1", help: "Leslie speed CC." },
    KeyDoc { block: "target.organ", key: "percussionCC", type_: "int -1..119", def: "-1", help: "Percussion CC." },
    KeyDoc { block: "target.organ", key: "chorusCC", type_: "int -1..119", def: "-1", help: "Chorus CC." },
    KeyDoc { block: "target.organ", key: "noteVelocity", type_: "int 1..127", def: "100", help: "Note velocity." },
    KeyDoc { block: "target.organ", key: "drawbarKeys", type_: "list of notes", def: "absent", help: "Upper drawbar keys." },
    KeyDoc { block: "target.organ", key: "lowerDrawbarKeys", type_: "list of notes", def: "absent", help: "Lower drawbar keys." },
    KeyDoc { block: "target.organ", key: "lowerManualChannel", type_: "int 0..16", def: "0", help: "Lower manual channel." },
    KeyDoc {
        block: "target.organ",
        key: "registrations",
        type_: "object gospel|jazz|rock|pad > 4 numbers",
        def: "absent",
        help: "Drawbar registrations.",
    },
    KeyDoc { block: "target.organ", key: "chorusValues", type_: "object off|V1|V2|V3|C1|C2|C3 > 0..127", def: "defaults", help: "Chorus CC values." },
    KeyDoc { block: "target.evolution", key: "range", type_: "2 notes", def: "[40,86]", help: "Playable range." },
    KeyDoc { block: "target.evolution", key: "articulations", type_: "object", def: "absent", help: "Articulation keys (see kEvArtNames)." },
    KeyDoc { block: "target.evolution", key: "stringKeys", type_: "up to 6 notes", def: "absent", help: "String-select keys." },
    KeyDoc { block: "target.evolution", key: "stringCount", type_: "int 1..6", def: "6", help: "String count." },
    KeyDoc { block: "target.evolution", key: "droneKey", type_: "int -1..127", def: "-1", help: "Drone key." },
    KeyDoc { block: "target.evolution", key: "droneNote", type_: "int -1..127", def: "-1", help: "Drone note." },
    KeyDoc { block: "target.evolution", key: "perfFxBase", type_: "int 0..127", def: "96", help: "Performance FX base." },
    KeyDoc { block: "target.evolution", key: "perfFx", type_: "object", def: "absent", help: "Performance FX keys (see kEvPerfFxNames)." },
    KeyDoc { block: "target.evolution", key: "vibratoCC", type_: "int -1..119", def: "1", help: "Vibrato CC." },
    KeyDoc { block: "target.evolution", key: "legatoST", type_: "int 0..12", def: "2", help: "Legato semitones." },
    KeyDoc { block: "target.evolution", key: "slideRange", type_: "int 0..12", def: "2", help: "Slide range." },
    KeyDoc { block: "target.evolution", key: "bypassLegatoKey", type_: "int -1..127", def: "-1", help: "Bypass legato key." },
    KeyDoc { block: "target.evolution", key: "strumKeys", type_: "[down,up] -1..127", def: "absent", help: "Strum keys." },
    KeyDoc { block: "target.evolution", key: "strumEngine", type_: "bool", def: "false", help: "Library strums itself." },
    KeyDoc { block: "target.evolution", key: "tremoloKeys", type_: "object {in,on,out}", def: "absent", help: "Tremolo keys." },
    KeyDoc { block: "target.evolution", key: "noteVelocity", type_: "2 numbers", def: "[30,124]", help: "Note velocity range." },
    KeyDoc { block: "target.evolution", key: "bendBlues", type_: "bool", def: "false", help: "Blues bends." },
    KeyDoc { block: "target.indiginusStrings", key: "range", type_: "2 notes", def: "[55,88]", help: "Playable range." },
    KeyDoc { block: "target.indiginusStrings", key: "layout", type_: "enum renegade|banjo", def: "renegade", help: "Layout." },
    KeyDoc { block: "target.indiginusStrings", key: "articulations", type_: "object", def: "absent", help: "Articulation keys (see kInArtNames)." },
    KeyDoc { block: "target.indiginusStrings", key: "neverSend", type_: "up to 4 notes", def: "absent", help: "Keys never sent." },
    KeyDoc { block: "target.indiginusStrings", key: "initKeys", type_: "up to 4 notes", def: "absent", help: "Keys sent at start." },
    KeyDoc { block: "target.indiginusStrings", key: "returnKey", type_: "int -1..127", def: "-1", help: "Return key." },
    KeyDoc {
        block: "target.indiginusStrings",
        key: "articulationReturn",
        type_: "object",
        def: "absent",
        help: "Return mode: unconfirmed, hold, latch, same.",
    },
    KeyDoc { block: "target.indiginusStrings", key: "expressionCC", type_: "int -1..119", def: "11", help: "Expression CC." },
    KeyDoc { block: "target.indiginusStrings", key: "vibratoCC", type_: "int -1..119", def: "-1", help: "Vibrato CC." },
    KeyDoc { block: "target.indiginusStrings", key: "masterKeyCC", type_: "int -1..119", def: "-1", help: "Master key CC." },
    KeyDoc { block: "target.indiginusStrings", key: "tremoloSpeedCC", type_: "int -1..119", def: "-1", help: "Tremolo speed CC." },
    KeyDoc { block: "target.indiginusStrings", key: "modeCC", type_: "object {cc}", def: "absent", help: "Mode CC." },
    KeyDoc {
        block: "target.indiginusStrings",
        key: "chordKeys",
        type_: "object",
        def: "absent",
        help: "down, up, rootSelectBase, major, minor, seventh.",
    },
    KeyDoc { block: "target.indiginusStrings", key: "slots", type_: "bool", def: "false", help: "Chord slots." },
    KeyDoc { block: "target.indiginusStrings", key: "droneNote", type_: "int -1..127", def: "-1", help: "Drone note." },
    KeyDoc { block: "target.indiginusStrings", key: "stringCount", type_: "int 1..6", def: "by layout", help: "String count." },
    KeyDoc { block: "target.brassSection", key: "range", type_: "2 notes", def: "[40,84]", help: "Playable range." },
    KeyDoc { block: "target.brassSection", key: "dynamicsCC", type_: "int 0..119", def: "1", help: "Dynamics CC." },
    KeyDoc { block: "target.brassSection", key: "noteVelocity", type_: "int 1..127", def: "100", help: "Note velocity." },
    KeyDoc { block: "target.brassSection", key: "setupCCs", type_: "object cc > value (max 4)", def: "absent", help: "CCs sent at setup." },
    KeyDoc { block: "target.brassSection", key: "legatoKeys", type_: "object {off,on}", def: "absent", help: "Legato keys." },
    KeyDoc { block: "target.brassSection", key: "instrumentCC", type_: "int -1..119", def: "15", help: "Instrument CC." },
    KeyDoc {
        block: "target.brassSection",
        key: "instrumentValues",
        type_: "object patch > value",
        def: "absent",
        help: "Instrument CC value per patch 1..26.",
    },
    KeyDoc {
        block: "target.brassSection",
        key: "instrumentFor",
        type_: "object {ensemble,melodic,low}",
        def: "absent",
        help: "Patch per part (21, 7, 22).",
    },
];
pub fn k_key_count() -> i32 {
    K_KEYS.len() as i32
}

/// The Library files to load: `("Players/Clay.json", bytes)`.
#[derive(Clone, Debug, Default)]
pub struct LibrarySource {
    pub files: Vec<(String, Vec<u8>)>,
    /// User files that could not be read (`"Players/X.json"`): named in the loader's errors.
    pub unreadable: Vec<String>,
}

impl LibrarySource {
    /// The factory Library embedded in the build.
    pub fn embedded() -> LibrarySource {
        LibrarySource { files: embedded::files().iter().map(|(p, b)| ((*p).to_string(), b.to_vec())).collect(), unreadable: Vec::new() }
    }

    /// The factory Library plus `dir/Players/*.json` and `dir/Targets/*.json`: a user file replaces the embedded file
    /// with the same name (compared ignoring case, as on MS's Windows Library folder). A file is read up to just past
    /// [`MAX_FILE_BYTES`], so the loader can name it as too large. A file that cannot be read (or whose name is not
    /// UTF-8) is listed in `unreadable` and replaces its embedded namesake with nothing. A missing folder adds nothing.
    pub fn with_user_folder(dir: &Path) -> LibrarySource {
        use std::io::Read as _;
        let mut src = LibrarySource::embedded();
        for sub in ["Players", "Targets"] {
            let Ok(rd) = std::fs::read_dir(dir.join(sub)) else { continue };
            let mut names: Vec<(String, bool, std::path::PathBuf)> = rd
                .flatten()
                .filter_map(|e| {
                    let p = e.path();
                    let os = e.file_name();
                    let json = p.extension().is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case("json"));
                    (json && p.is_file()).then(|| (os.to_string_lossy().into_owned(), os.to_str().is_some(), p))
                })
                .collect();
            names.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, utf8, path) in names {
                let key = format!("{sub}/{name}");
                let mut bytes = Vec::new();
                let read = utf8 && std::fs::File::open(&path).and_then(|f| f.take(MAX_FILE_BYTES as u64 + 1).read_to_end(&mut bytes)).is_ok();
                if !read {
                    src.files.retain(|(p, _)| !p.eq_ignore_ascii_case(&key));
                    src.unreadable.push(key);
                    continue;
                }
                match src.files.iter_mut().find(|(p, _)| p.eq_ignore_ascii_case(&key)) {
                    Some(slot) => *slot = (key, bytes),
                    None => src.files.push((key, bytes)),
                }
            }
        }
        src
    }

    /// The direct children of `dir` ("Players"), sorted by file name ignoring case (`jsonFiles`).
    fn json_files(&self, dir: &str) -> Vec<(&str, &[u8])> {
        let mut v: Vec<(&str, &[u8])> = self
            .files
            .iter()
            .filter_map(|(p, b)| {
                let name = p.strip_prefix(dir)?.strip_prefix('/')?;
                let json = name.len() > 5 && name.get(name.len() - 5..).is_some_and(|x| x.eq_ignore_ascii_case(".json"));
                (json && !name.contains('/')).then_some((name, b.as_slice()))
            })
            .collect();
        v.sort_by(|a, b| compare_ignore_case(a.0, b.0));
        v
    }
}

/// A song's own copies of players and targets (JSON text), played in this instance only (Phase 5a).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LibraryOverlay {
    pub players: Vec<String>,
    pub targets: Vec<String>,
}

impl LibraryOverlay {
    pub fn empty(&self) -> bool {
        self.players.is_empty() && self.targets.is_empty()
    }
}

/// The indiginusStrings targets as the plucked engine sees them (Musician ev*, Task 4): range, velocity 50-110
/// (below 40 only for deliberate mutes), strings and their open tuning, the drone (dropped when outside the range),
/// the vibrato CC; the articulation keys only when the return mode is confirmed (unconfirmed: none is ever sent).
fn indiginus_plucked_view(t: &mut TargetData) {
    t.ev_low = t.in_low;
    t.ev_high = t.in_high;
    t.ev_vel_lo = 50;
    t.ev_vel_hi = 110;
    t.ev_string_count = t.in_string_count;
    if t.in_layout == 1 {
        t.ev_open = [50, 55, 59, 62, 67, -1]; // open G, the 5th string's drone on top
    } else if t.in_string_count == 6 {
        t.ev_open = [43, 47, 50, 55, 59, 62]; // resonator: open G (GBDGBD)
    } else if t.in_low >= 60 {
        t.ev_open = [60, 64, 67, 69, -1, -1]; // ukulele GCEA (Hi-G, sorted)
    } else {
        t.ev_open = [55, 62, 69, 76, -1, -1]; // mandolin GDAE
    }
    t.ev_drone_key = -1;
    t.ev_drone_note = if t.in_drone_note >= t.in_low && t.in_drone_note <= t.in_high { t.in_drone_note } else { -1 };
    t.ev_vibrato_cc = t.in_vibrato_cc;
    t.ev_legato_st = 2;
    t.ev_slide_range = 2;
    t.ev_bypass_legato_key = -1;
    t.ev_strum_down = -1;
    t.ev_strum_up = -1;
    t.ev_strum_engine = false;
    t.ev_bend_blues = false;
    t.ev_string_keys = [-1; 6];
    t.ev_art = [-1; EV_ART_COUNT];
    t.ev_trem_in = -1;
    t.ev_trem_on = -1;
    t.ev_trem_out = -1;
    if t.in_return_mode == 0 {
        return;
    }
    let art = t.in_art;
    t.ev_art[EV_MUTE] = art[IN_MUTES];
    t.ev_art[EV_GRACE_UP] = if art[IN_GRACE] >= 0 { art[IN_GRACE] } else { art[IN_GRACE_SLIDE_UP] };
    t.ev_art[EV_GRACE_DOWN] = art[IN_GRACE_SLIDE_DOWN];
    t.ev_art[EV_BUZZ_TRILL] = if art[IN_TRILL_UP] >= 0 { art[IN_TRILL_UP] } else { art[IN_TRILL_DOWN] };
    t.ev_art[EV_VELOCITY_SLIDE] = if art[IN_UP_SLIDE] >= 0 { art[IN_UP_SLIDE] } else { art[IN_DOWN_SLIDE] };
    t.ev_trem_on = art[IN_TREMOLO];
}

struct Ctx<'a> {
    /// "Players/Clay.json"
    label: &'a str,
    errors: &'a mut Vec<String>,
    ok: bool,
}

impl Ctx<'_> {
    fn fail(&mut self, what: impl std::fmt::Display) {
        self.errors.push(format!("{}: {what}", self.label));
        self.ok = false;
    }
}

fn index_of(names: &[&str], s: &str) -> i32 {
    names.iter().position(|n| *n == s).map_or(-1, |i| i as i32)
}

/// Integer field in [lo, hi]. Missing or wrong type is an error unless `optional_default` is given.
fn get_int(c: &mut Ctx, obj: &Var, key: &str, lo: i32, hi: i32, optional_default: Option<i32>) -> i32 {
    let v = obj.prop(key);
    if v.is_void()
        && let Some(d) = optional_default
    {
        return d;
    }
    if let Var::Bool(b) = v {
        return i32::from(*b);
    }
    if !v.is_number() {
        c.fail(format!("{key} must be a number"));
        return lo;
    }
    let d = v.to_double();
    if d != d.floor() {
        c.fail(format!("{key} must be a whole number (got {})", fmt_g(d)));
        return lo;
    }
    if d < f64::from(lo) || d > f64::from(hi) {
        c.fail(format!("{key} must be {lo}..{hi} (got {})", fmt_g(d)));
        return lo;
    }
    d as i32
}

fn get_float(c: &mut Ctx, obj: &Var, key: &str, lo: f32, hi: f32) -> f32 {
    let v = obj.prop(key);
    if !v.is_number() {
        c.fail(format!("{key} must be a number"));
        return lo;
    }
    let d = v.to_double();
    if !d.is_finite() || d < f64::from(lo) || d > f64::from(hi) {
        c.fail(format!("{key} must be {}..{} (got {})", fmt_g(f64::from(lo)), fmt_g(f64::from(hi)), fmt_g(d)));
        return lo;
    }
    d as f32 // JSON doubles -> float, exactly as the Cmajor float literals were parsed
}

fn get_bool(obj: &Var, key: &str, def: bool) -> bool {
    let v = obj.prop(key);
    if let Var::Bool(b) = v {
        return *b;
    }
    if v.is_number() {
        return v.to_double() != 0.0;
    }
    def
}

/// Every property of `obj` not in `known` fails as "<block>.<key> is not a known setting (see README.md)".
fn check_known(c: &mut Ctx, obj: &[(String, Var)], known: &[&str], block: &str) {
    for (name, _) in obj {
        if !known.contains(&name.as_str()) {
            c.fail(format!("{block}.{name} is not a known setting (see README.md)"));
        }
    }
}

/// `if (obj.hasProperty (k)) out = getFloat (c, obj, k, 0.0f, 1.0f);`
fn amount(c: &mut Ctx, obj: &Var, k: &str, out: &mut f32) {
    if obj.has(k) {
        *out = get_float(c, obj, k, 0.0, 1.0);
    }
}

// ---- Players ------------------------------------------------------------------------------

/// One `grids` / `grids12` object (MS has the two loops written out; they differ only in these parameters).
struct GridShape<'a> {
    what: &'a str,
    steps: i32,
    /// steps per lane (both bars)
    stride: i32,
}

fn parse_grids(c: &mut Ctx, gobj: &[(String, Var)], shape: &GridShape, lane_ok: &dyn Fn(&str, &str) -> bool, out: &mut [i32]) {
    let what = shape.what;
    for (ename, ev) in gobj {
        let e = if ename.chars().count() == 2 && ename.starts_with('E') { get_trailing_int_value(ename) } else { 0 };
        if !(1..=4).contains(&e) {
            c.fail(format!("{what}: \"{ename}\" is not E1..E4"));
            continue;
        }
        let Some(lobj) = ev.dyn_obj() else {
            c.fail(format!("{what}.{ename} must be an object"));
            continue;
        };
        for (lname, lv) in lobj {
            let lane = if lname.starts_with("lane") && lname.chars().count() == 5 { get_trailing_int_value(lname) } else { -1 };
            if !(0..=3).contains(&lane) {
                c.fail(format!("{what}.{ename}: \"{lname}\" is not lane0..lane3"));
                continue;
            }
            if !lane_ok(ename, lname) {
                c.fail(format!("{what}.{ename}.{lname} is not a lane of grids"));
                continue;
            }
            let Some(bobj) = lv.dyn_obj() else {
                c.fail(format!("{what}.{ename}.{lname} must be an object"));
                continue;
            };
            for (bname, bv) in bobj {
                let bar = match bname.as_str() {
                    "barA" => 0,
                    "barB" => 1,
                    _ => -1,
                };
                let place = format!("{what}.{ename}.{lname}.{bname}");
                if bar < 0 {
                    c.fail(format!("{what}.{ename}.{lname}: \"{bname}\" is not barA or barB"));
                    continue;
                }
                let Some(hits) = bv.array() else {
                    c.fail(format!("{place} must be a list of hits"));
                    continue;
                };
                let mut seen = [false; 16];
                for h in hits {
                    let a = match h.array() {
                        Some(a) if a.len() == 5 => a,
                        _ => {
                            c.fail(format!("{place}: each hit must be [step, type, prob, vel, len]"));
                            continue;
                        }
                    };
                    let mut v = [0_i32; 5];
                    let mut numeric = true;
                    for (k, x) in a.iter().enumerate() {
                        if !x.is_number() {
                            numeric = false;
                        } else {
                            v[k] = x.to_double() as i32;
                        }
                    }
                    if !numeric {
                        c.fail(format!("{place}: hit values must be numbers"));
                        continue;
                    }
                    let hit_text = format!("{place} hit [{},{},{},{},{}]", v[0], v[1], v[2], v[3], v[4]);
                    if v[0] < 0 || v[0] > shape.steps - 1 {
                        c.fail(format!("{hit_text}: step must be 0..{}", shape.steps - 1));
                        continue;
                    }
                    if v[1] < 1 || v[1] > 127 {
                        c.fail(format!("{hit_text}: type must be 1..127"));
                        continue;
                    }
                    if v[2] < 1 || v[2] > 100 {
                        c.fail(format!("{hit_text}: prob must be 1..100"));
                        continue;
                    }
                    if v[3] < 1 || v[3] > 127 {
                        c.fail(format!("{hit_text}: vel must be 1..127"));
                        continue;
                    }
                    if v[4] < 1 || v[4] > shape.steps {
                        c.fail(format!("{hit_text}: len must be 1..{}", shape.steps));
                        continue;
                    }
                    let Some(s) = seen.get_mut(v[0] as usize) else { continue };
                    if *s {
                        c.fail(format!("{hit_text}: two hits on step {}", v[0]));
                        continue;
                    }
                    *s = true;
                    let base = ((((e - 1) * 4 + lane) * shape.stride) + bar * shape.steps + v[0]) * 4;
                    if let Some(cell) = out.get_mut(base as usize..base as usize + 4) {
                        cell.copy_from_slice(&[v[2], v[3], v[4], v[1]]);
                    }
                }
            }
        }
    }
    for e in 1..=4 {
        let lane_ints = (4 * shape.stride) as usize;
        let from = (e - 1) * lane_ints;
        let any = (0..lane_ints).any(|k| out.get((from + k) * 4).is_some_and(|&p| p > 0));
        if !any {
            c.fail(format!("{what}.E{e} has no hits: every energy needs at least one lane"));
        }
    }
}

fn parse_persona(c: &mut Ctx, j: &Var) -> Option<Box<PersonaData>> {
    let mut p = Box::<PersonaData>::default();
    if !j.is_object() {
        c.fail("not a JSON object");
        return None;
    }
    if j.prop_int("format", 0) != 1 {
        c.fail("format must be 1");
    }
    p.name = j.prop_str("name", "").trim().to_string();
    if p.name.is_empty() {
        c.fail("name is missing");
    } else if p.name.to_lowercase() == "session" {
        c.fail("the name \"Session\" is reserved (it means no player): choose another name");
    }
    p.desc = j.prop_str("desc", "");
    p.seed = get_int(c, j, "seed", 1, K_MAX_SEED as i32, None);

    let role = j.prop_str("role", "");
    p.role = index_of(&ROLE_NAMES, &role);
    if p.role < 0 {
        c.fail(format!("role \"{role}\" is not one of keys, bass, drums, guitar, ensemble, melodic"));
    }
    let style = j.prop_str("style", "");
    p.voicing = index_of(&STYLE_NAMES, &style);
    if p.voicing < 0 {
        c.fail(format!("style \"{style}\" is not a known style (see README.md)"));
    }

    let flags = j.prop("flags");
    if let Some(fo) = flags.dyn_obj() {
        for (name, _) in fo {
            if name != "heavy" && name != "slap" && name != "artic" {
                c.fail(format!("flags.{name} is not a known setting (see README.md)"));
            }
        }
    }
    p.heavy = i32::from(get_bool(flags, "heavy", false));
    p.slap = get_bool(flags, "slap", false);
    p.artic = if flags.is_object() { get_int(c, flags, "artic", 0, 4095, Some(0)) } else { 0 };

    if let Some(slots) = j.prop("slots").array() {
        for s in slots {
            let inst = s.prop_str("instrument", "");
            let i = index_of(&INSTRUMENT_NAMES, &inst);
            if i < 0 {
                c.fail(format!("slots: unknown instrument \"{inst}\""));
                continue;
            }
            let n = get_int(c, s, "slot", 1, 9999, Some(0)); // optional: an ordering hint (and the Amorph slot when 1..16)
            p.slots.push((i, n));
        }
        if slots.is_empty() {
            c.fail("slots is empty: the player would never be playable");
        }
    } else {
        c.fail("slots is missing");
    }

    let tags = j.prop("tags");
    if let Some(ta) = tags.array() {
        for t in ta {
            if t.is_string() && !t.to_jstring().trim().is_empty() {
                p.tags.push(t.to_jstring().trim().to_lowercase());
            } else {
                c.fail("tags must be a list of words");
                break;
            }
        }
    } else if !tags.is_void() {
        c.fail("tags must be a list of words");
    }

    let ample = j.prop("ample");
    if let Some(ao) = ample.dyn_obj() {
        const KNOWN: [&str; 15] = [
            "mode",
            "openStringsFirst",
            "positionMode",
            "palmMute",
            "pops",
            "harmonics",
            "hammerOn",
            "slides",
            "slideIn",
            "fx",
            "muted",
            "bassStrings",
            "strumNoise",
            "capo",
            "strumInversion",
        ];
        check_known(c, ao, &KNOWN, "ample");
        let a = &mut p.ample;
        let mode = ample.prop("mode");
        let mode_s = mode.to_jstring();
        if mode.is_string() && mode_s == "strum" {
            a.mode = 0;
        } else if mode.is_string() && mode_s == "notes" {
            a.mode = 1;
        } else if mode.is_string() && mode_s == "auto" {
            a.mode = 2;
        } else if !mode.is_void() {
            c.fail("ample.mode must be strum, notes or auto");
        }
        let pos = ample.prop("positionMode");
        let pos_s = pos.to_jstring();
        if pos.is_string() && pos_s == "chord" {
            a.position_mode = 0;
        } else if pos.is_string() && pos_s == "melody" {
            a.position_mode = 1;
        } else if !pos.is_void() {
            c.fail("ample.positionMode must be chord or melody");
        }
        a.open_strings_first = get_bool(ample, "openStringsFirst", false);
        a.bass_strings = get_bool(ample, "bassStrings", true);
        amount(c, ample, "palmMute", &mut a.palm_mute);
        amount(c, ample, "pops", &mut a.pops);
        amount(c, ample, "harmonics", &mut a.harmonics);
        amount(c, ample, "hammerOn", &mut a.hammer_on);
        amount(c, ample, "slides", &mut a.slides);
        amount(c, ample, "slideIn", &mut a.slide_in);
        amount(c, ample, "fx", &mut a.fx);
        amount(c, ample, "muted", &mut a.muted);
        amount(c, ample, "strumNoise", &mut a.strum_noise);
        if ample.has("capo") {
            a.capo = get_int(c, ample, "capo", 0, 7, None);
        }
        if ample.has("strumInversion") {
            a.strum_inversion = get_int(c, ample, "strumInversion", 0, 2, None);
        }
    } else if !ample.is_void() {
        c.fail("ample must be an object");
    }

    let fiddle = j.prop("fiddle");
    if let Some(fo) = fiddle.dyn_obj() {
        const KNOWN: [&str; 13] = [
            "scoops",
            "graces",
            "graceSlides",
            "trills",
            "slides",
            "doubleStops",
            "shuffle",
            "chop",
            "kickoff",
            "fillLicks",
            "turnaround",
            "tag",
            "shuffleStyle",
        ];
        let f = &mut p.fiddle;
        let out: [&mut f32; 12] = [
            &mut f.scoops,
            &mut f.graces,
            &mut f.grace_slides,
            &mut f.trills,
            &mut f.slides,
            &mut f.double_stops,
            &mut f.shuffle,
            &mut f.chop,
            &mut f.kickoff,
            &mut f.fill_licks,
            &mut f.turnaround,
            &mut f.tag,
        ];
        check_known(c, fo, &KNOWN, "fiddle");
        for (k, o) in KNOWN.iter().zip(out) {
            amount(c, fiddle, k, o);
        }
        if fiddle.has("shuffleStyle") {
            let s = fiddle.prop("shuffleStyle").to_jstring();
            match s.as_str() {
                "nashville" => p.fiddle.shuffle_style = 0,
                "georgia" => p.fiddle.shuffle_style = 1,
                "longbow" => p.fiddle.shuffle_style = 2,
                _ => c.fail(format!("fiddle.shuffleStyle \"{s}\" is not nashville, georgia or longbow")),
            }
        }
    } else if !fiddle.is_void() {
        c.fail("fiddle must be an object");
    }

    let bass = j.prop("bass");
    if let Some(bo) = bass.dyn_obj() {
        const KNOWN: [&str; 5] = ["vibrato", "letRing", "falls", "position", "pluck"];
        check_known(c, bo, &KNOWN, "bass");
        let b = &mut p.bass;
        let out: [&mut f32; 3] = [&mut b.vibrato, &mut b.let_ring, &mut b.falls];
        for (k, o) in KNOWN.iter().zip(out) {
            amount(c, bass, k, o);
        }
        let pick = |c: &mut Ctx, key: &str, names: &[&str], out: &mut i32| {
            if !bass.has(key) {
                return;
            }
            let v = bass.prop(key).to_jstring();
            if let Some(i) = names.iter().position(|n| *n == v) {
                *out = i as i32;
                return;
            }
            c.fail(format!("bass.{key} must be one of {}", names.join(", ")));
        };
        pick(c, "position", &["low", "mid", "high"], &mut b.position);
        pick(c, "pluck", &["neck", "mid", "bridge"], &mut b.pluck);
    } else if !bass.is_void() {
        c.fail("bass must be an object");
    }

    let drums = j.prop("drums");
    if let Some(dobj) = drums.dyn_obj() {
        const KNOWN: [&str; 9] = ["crossStick", "pedalHat", "flams", "hatBark", "crashAlternate", "aperture", "china", "rideSwitch", "bellCymbal"];
        let d = &mut p.drums;
        let out: [&mut f32; 9] = [
            &mut d.cross_stick,
            &mut d.pedal_hat,
            &mut d.flams,
            &mut d.hat_bark,
            &mut d.crash_alternate,
            &mut d.aperture,
            &mut d.china,
            &mut d.ride_switch,
            &mut d.bell_cymbal,
        ];
        check_known(c, dobj, &KNOWN, "drums");
        for (k, o) in KNOWN.iter().zip(out) {
            amount(c, drums, k, o);
        }
    } else if !drums.is_void() {
        c.fail("drums must be an object");
    }

    let violin = j.prop("violin");
    if let Some(vo) = violin.dyn_obj() {
        const KNOWN: [&str; 4] = ["portamento", "doubleStops", "vibratoDepth", "rebow"];
        let v = &mut p.violin;
        let out: [&mut f32; 4] = [&mut v.portamento, &mut v.double_stops, &mut v.vibrato_depth, &mut v.rebow];
        check_known(c, vo, &KNOWN, "violin");
        for (k, o) in KNOWN.iter().zip(out) {
            amount(c, violin, k, o);
        }
    } else if !violin.is_void() {
        c.fail("violin must be an object");
    }

    let organ = j.prop("organ");
    if let Some(oo) = organ.dyn_obj() {
        const KNOWN: [&str; 9] = ["percussion", "swell", "leslieFast", "chorus", "build", "leslieKick", "smear", "lowerHand", "registration"];
        check_known(c, oo, &KNOWN, "organ");
        let o = &mut p.organ;
        let out: [&mut f32; 3] = [&mut o.percussion, &mut o.swell, &mut o.leslie_fast];
        for (k, x) in KNOWN.iter().zip(out) {
            amount(c, organ, k, x);
        }
        let out2: [&mut f32; 4] = [&mut o.build, &mut o.leslie_kick, &mut o.smear, &mut o.lower_hand];
        for (k, x) in KNOWN[4..].iter().zip(out2) {
            amount(c, organ, k, x);
        }
        if organ.has("registration") {
            const REGS: [&str; 4] = ["gospel", "jazz", "rock", "pad"];
            let i = index_of(&REGS, &organ.prop("registration").to_jstring());
            if i < 0 {
                c.fail("organ.registration must be one of gospel, jazz, rock, pad");
            } else {
                o.registration = i;
            }
        }
        if organ.has("chorus") {
            const NAMES: [&str; 7] = ["off", "V1", "V2", "V3", "C1", "C2", "C3"];
            let i = index_of(&NAMES, &organ.prop("chorus").to_jstring());
            if i < 0 {
                c.fail("organ.chorus must be one of off, V1, V2, V3, C1, C2, C3");
            } else {
                o.chorus = i;
            }
        }
    } else if !organ.is_void() {
        c.fail("organ must be an object");
    }

    let plucked = j.prop("plucked");
    if let Some(po) = plucked.dyn_obj() {
        const KNOWN: [&str; 19] = [
            "mode",
            "strumMs",
            "upWeight",
            "upStrings",
            "chop",
            "tremolo",
            "rolls",
            "clawhammer",
            "dropThumb",
            "hammer",
            "slides",
            "graces",
            "buzz",
            "palmMute",
            "harmonics",
            "vibrato",
            "kickoff",
            "fillLicks",
            "turnaround",
        ];
        check_known(c, po, &KNOWN, "plucked");
        let pl = &mut p.plucked;
        if plucked.has("mode") {
            const MODES: [&str; 3] = ["auto", "strum", "notes"];
            let i = index_of(&MODES, &plucked.prop("mode").to_jstring());
            if i < 0 {
                c.fail("plucked.mode must be auto, strum or notes");
            } else {
                pl.mode = i;
            }
        }
        if plucked.has("strumMs") {
            pl.strum_ms = get_int(c, plucked, "strumMs", 0, 100, None);
        }
        if plucked.has("upStrings") {
            pl.up_strings = get_int(c, plucked, "upStrings", 1, 6, None);
        }
        if plucked.has("rolls") {
            pl.rolls = get_int(c, plucked, "rolls", 0, 4, None);
        }
        const FK: [&str; 15] = [
            "upWeight",
            "chop",
            "tremolo",
            "clawhammer",
            "dropThumb",
            "hammer",
            "slides",
            "graces",
            "buzz",
            "palmMute",
            "harmonics",
            "vibrato",
            "kickoff",
            "fillLicks",
            "turnaround",
        ];
        let fo: [&mut f32; 15] = [
            &mut pl.up_weight,
            &mut pl.chop,
            &mut pl.tremolo,
            &mut pl.clawhammer,
            &mut pl.drop_thumb,
            &mut pl.hammer,
            &mut pl.slides,
            &mut pl.graces,
            &mut pl.buzz,
            &mut pl.palm_mute,
            &mut pl.harmonics,
            &mut pl.vibrato,
            &mut pl.kickoff,
            &mut pl.fill_licks,
            &mut pl.turnaround,
        ];
        for (k, o) in FK.iter().zip(fo) {
            amount(c, plucked, k, o);
        }
    } else if !plucked.is_void() {
        c.fail("plucked must be an object");
    }

    let brass = j.prop("brass");
    if let Some(bo) = brass.dyn_obj() {
        const KNOWN: [&str; 6] = ["stab", "swell", "voiced", "fall", "legato", "double"];
        for (name, _) in bo {
            if index_of(&KNOWN, name) < 0 {
                c.fail(format!("brass.{name} is not a known setting (see README.md)"));
            }
        }
        let b = &mut p.brass;
        let out: [&mut f32; 5] = [&mut b.stab, &mut b.swell, &mut b.voiced, &mut b.fall, &mut b.legato];
        for (k, o) in KNOWN.iter().zip(out) {
            amount(c, brass, k, o);
        }
        if brass.has("double") {
            match brass.prop("double").to_jstring().as_str() {
                "none" => b.doubling = 0,
                "octave" => b.doubling = 1,
                "thirds" => b.doubling = 2,
                _ => c.fail("brass.double must be octave, thirds or none"),
            }
        }
    } else if !brass.is_void() {
        c.fail("brass must be an object");
    }

    let groove = j.prop("groove");
    if let Some(go) = groove.dyn_obj() {
        const KNOWN: [&str; 6] = ["timing", "timing12", "push", "pickups", "variation", "space"];
        check_known(c, go, &KNOWN, "groove");
        let g = &mut p.groove;
        let ms_list = |c: &mut Ctx, key: &str, out: &mut [f32], what: &str| {
            let v = groove.prop(key);
            match v.array() {
                Some(a) if a.len() == out.len() => {
                    for (x, o) in a.iter().zip(out.iter_mut()) {
                        if !x.is_number() || !x.to_double().is_finite() || x.to_double() < -30.0 || x.to_double() > 30.0 {
                            c.fail(format!("groove.{key} values must be -30..30 (ms)"));
                            break;
                        }
                        *o = x.to_double() as f32;
                    }
                }
                _ if !v.is_void() => c.fail(format!("groove.{key} must be {} numbers ({what})", out.len())),
                _ => {}
            }
        };
        ms_list(c, "timing", &mut g.timing, "ms per 16th");
        ms_list(c, "timing12", &mut g.timing12, "ms per step");
        amount(c, groove, "push", &mut g.push);
        amount(c, groove, "pickups", &mut g.pickups);
        amount(c, groove, "variation", &mut g.variation);
        amount(c, groove, "space", &mut g.space);
    } else if !groove.is_void() {
        c.fail("groove must be an object");
    }

    let feel = j.prop("feel");
    if !feel.is_object() {
        c.fail("feel is missing");
    } else {
        p.swing_base = get_float(c, feel, "swingBase", 0.0, 100.0);
        p.swing_steps = get_int(c, feel, "swingSteps", 1, 16, None);
        p.timing_ms = get_float(c, feel, "timingMs", -100.0, 100.0);
        p.roll_ms = get_float(c, feel, "rollMs", 0.0, 200.0);
        p.jitter_ms = get_float(c, feel, "jitterMs", 0.0, 100.0);
        p.vel_var = get_float(c, feel, "velVar", 0.0, 64.0);
        p.arc = get_float(c, feel, "arc", 0.0, 1.0);
        p.pedal_mode = get_int(c, feel, "pedalMode", 0, 6, None);
        p.fill_rate = get_float(c, feel, "fillRate", 0.0, 1.0);
        p.fill_count = get_int(c, feel, "fillCount", 0, 16, None);
        p.rh_low = get_int(c, feel, "rhLow", 0, 127, None);
        p.rh_high = get_int(c, feel, "rhHigh", 0, 127, None);
        p.rh_anchor = get_int(c, feel, "rhAnchor", 0, 127, None);
        p.lh_low = get_int(c, feel, "lhLow", 0, 127, None);
        match feel.prop("laneMs").array() {
            Some(lane) if lane.len() == 4 => {
                for (x, o) in lane.iter().zip(p.lane_ms.iter_mut()) {
                    if !x.is_number() {
                        c.fail("feel.laneMs must be numbers");
                        break;
                    }
                    *o = x.to_double() as f32;
                }
            }
            _ => c.fail("feel.laneMs must be 4 numbers"),
        }
    }

    let acc = j.prop("accents").array();
    let only12_file = j.has("grids12") && !j.has("grids");
    match acc {
        None if only12_file => p.accents = [1.0; 16], // a 12-only player needn't carry 16 accents
        Some(acc) if acc.len() == 16 => {
            for (i, (v, o)) in acc.iter().zip(p.accents.iter_mut()).enumerate() {
                if !v.is_number() || v.to_double() < 0.25 || v.to_double() > 2.0 {
                    c.fail(format!("accents[{i}] must be a number 0.25..2"));
                    break;
                }
                *o = v.to_double() as f32;
            }
        }
        _ => c.fail(format!("accents must be exactly 16 numbers (got {})", acc.map_or(0, <[Var]>::len))),
    }

    p.accents12 = [1.0; 12];
    if j.has("accents12") {
        match j.prop("accents12").array() {
            Some(acc12) if acc12.len() == 12 => {
                for (i, (v, o)) in acc12.iter().zip(p.accents12.iter_mut()).enumerate() {
                    if !v.is_number() || v.to_double() < 0.25 || v.to_double() > 2.0 {
                        c.fail(format!("accents12[{i}] must be a number 0.25..2"));
                        break;
                    }
                    *o = v.to_double() as f32;
                }
            }
            acc12 => c.fail(format!("accents12 must be exactly 12 numbers (got {})", acc12.map_or(0, <[Var]>::len))),
        }
    }

    let grids = j.prop("grids");
    let has_grids12 = j.has("grids12");
    if let Some(gobj) = grids.dyn_obj() {
        parse_grids(c, gobj, &GridShape { what: "grids", steps: 16, stride: 32 }, &|_, _| true, &mut p.grid);
    } else if !has_grids12 {
        c.fail("grids is missing");
    } else if !grids.is_void() {
        c.fail("grids must be an object");
    }

    // Native 12-step grids (6/8, 3/4): the same shape as grids, steps 0..11; lanes must be ones grids has.
    if has_grids12 {
        let g12 = j.prop("grids12");
        p.native12 = true;
        p.only12 = !grids.is_object();
        let only12 = p.only12;
        let lane_in16 = |e: &str, lane: &str| only12 || grids.prop(e).prop(lane).is_object();
        if let Some(gobj12) = g12.dyn_obj() {
            parse_grids(c, gobj12, &GridShape { what: "grids12", steps: 12, stride: 24 }, &lane_in16, &mut p.grids12);
        } else {
            c.fail("grids12 must be an object");
        }

        p.meters12 = 3; // both, unless the file lists them
        if j.has("meters") {
            match j.prop("meters").array() {
                Some(ms) if !ms.is_empty() => {
                    let mut mask: u8 = 0;
                    for m in ms {
                        let name = m.to_jstring();
                        if name == "6/8" {
                            mask |= 1;
                        } else if name == "3/4" {
                            mask |= 2;
                        } else {
                            c.fail(format!("meters: \"{name}\" is not 6/8 or 3/4"));
                        }
                    }
                    p.meters12 = mask;
                }
                _ => c.fail("meters must be a non-empty list of \"6/8\" or \"3/4\""),
            }
        }
    } else if j.has("meters") {
        c.fail("meters needs grids12");
    }

    // Auto (Ample): per Energy, does the strumming hand (lane 1) strum (types 1-3) more than it picks (4+)?
    for e in 0..4 {
        let (mut strums, mut picks) = (0, 0);
        for st in 0..32 {
            let base = (((e * 4 + 1) * 32) + st) * 4;
            let prob = p.grid.get(base).copied().unwrap_or(0);
            let typ = p.grid.get(base + 3).copied().unwrap_or(0);
            if prob <= 0 {
                continue;
            }
            if (1..=3).contains(&typ) {
                strums += prob;
            } else if typ >= 4 {
                picks += prob;
            }
        }
        p.ample.strum_at[e] = strums >= picks;
    }
    if !c.ok {
        return None;
    }
    Some(p)
}

// ---- Targets ------------------------------------------------------------------------------

/// Every top-level target "data" key each driver's case below reads: anything else is rejected.
/// Read from K_KEYS (LibraryTest checks it against the cases in parseTarget).
fn target_data_keys(d: usize) -> impl Iterator<Item = &'static str> {
    let block = format!("target.{}", DRIVER_NAMES.get(d).copied().unwrap_or(""));
    K_KEYS.iter().filter(move |k| k.block == block).map(|k| k.key)
}

const DRIVERS: [Driver; K_DRIVERS] = [
    Driver::Generic,
    Driver::DrumMap,
    Driver::KeyswitchBass,
    Driver::PianoPedal,
    Driver::Swam,
    Driver::ArticulationKeys,
    Driver::UjamPhrase,
    Driver::AmpleGuitar,
    Driver::Fiddle,
    Driver::Organ,
    Driver::Evolution,
    Driver::IndiginusStrings,
    Driver::BrassSection,
    Driver::Keyswitch,
];

/// `intArray`: absent keeps `out`; else exactly `out.len()` numbers 0..127.
fn int_array(c: &mut Ctx, data: &Var, key: &str, out: &mut [i32]) {
    let v = data.prop(key);
    if v.is_void() {
        return;
    }
    let a = match v.array() {
        Some(a) if a.len() == out.len() => a,
        _ => {
            c.fail(format!("data.{key} must be {} numbers", out.len()));
            return;
        }
    };
    for (x, o) in a.iter().zip(out.iter_mut()) {
        if !x.is_number() || x.to_int() < 0 || x.to_int() > 127 {
            c.fail(format!("data.{key} values must be 0..127"));
            return;
        }
        *o = x.to_int();
    }
}

/// A whole-number note 0..127 (the checks the ampleGuitar / keyswitch cases spell out).
fn is_note(v: &Var) -> bool {
    v.is_number() && v.to_double() == v.to_double().floor() && v.to_int() >= 0 && v.to_int() <= 127
}

#[allow(clippy::manual_range_contains)] // MS's comparisons, NaN included
fn parse_target(c: &mut Ctx, j: &Var) -> Option<Box<TargetData>> {
    let mut t = Box::<TargetData>::default();
    if !j.is_object() {
        c.fail("not a JSON object");
        return None;
    }
    if j.prop_int("format", 0) != 1 {
        c.fail("format must be 1");
    }
    t.name = j.prop_str("name", "").trim().to_string();
    if t.name.is_empty() {
        c.fail("name is missing");
    }
    t.help = j.prop_str("help", "");
    t.slot = get_int(c, j, "slot", 0, K_TARGET_SLOTS as i32 - 1, None);
    let drv = j.prop_str("driver", "");
    let d = index_of(&DRIVER_NAMES, &drv);
    let Some(&driver) = usize::try_from(d).ok().and_then(|i| DRIVERS.get(i)) else {
        c.fail(format!("driver \"{drv}\" is not a known driver (see README.md)"));
        return None;
    };
    t.driver = driver;
    t.ks_octave_default = get_int(c, j, "ksOctaveDefault", -2, 2, Some(0));
    // Velocity calibration (Natural feel): { "lo", "hi", "curve" }; absent = velocity as played.
    let vel = j.prop("velocity");
    if !vel.is_void() {
        let lo = get_int(c, vel, "lo", 1, 127, None);
        let hi = get_int(c, vel, "hi", 1, 127, None);
        let g = if vel.has("curve") { vel.prop("curve").to_double() } else { 1.0 };
        if lo >= hi {
            c.fail("velocity: lo must be below hi");
        } else if g < 0.25 || g > 4.0 {
            c.fail("velocity: curve must be 0.25..4");
        } else {
            t.vel_lo = lo;
            t.vel_hi = hi;
            t.vel_curve = g as f32;
        }
    }
    if let Some(insts) = j.prop("instruments").array() {
        for v in insts {
            let name = v.to_jstring();
            let i = index_of(&INSTRUMENT_NAMES, &name);
            match t.instruments.get_mut(i.max(0) as usize) {
                Some(slot) if i >= 0 => *slot = true,
                _ => c.fail(format!("instruments: unknown instrument \"{name}\"")),
            }
        }
    } else {
        c.fail("instruments is missing");
    }

    let data = j.prop("data");
    if let Some(dobj) = data.dyn_obj() {
        for (key, _) in dobj {
            if !target_data_keys(d as usize).any(|k| k == key) {
                c.fail(format!("{key} is not a known target setting for driver {drv} (see README.md)"));
            }
        }
    }
    let num = |c: &mut Ctx, key: &str, lo: i32, hi: i32, def: i32| get_int(c, data, key, lo, hi, Some(def));

    match t.driver {
        Driver::Generic => {}
        Driver::DrumMap => {
            if let Some(rules) = data.prop("rules").array() {
                for r in rules {
                    let mut dr = DrumRule { note: get_int(c, r, "note", 0, 127, None), ..DrumRule::default() };
                    dr.min_vel = get_int(c, r, "minVel", 0, 127, Some(-100000));
                    dr.max_vel = get_int(c, r, "maxVel", 0, 127, Some(100000));
                    let side = r.prop_str("side", "any");
                    dr.side = match side.as_str() {
                        "left" => 1,
                        "right" => 2,
                        _ => 0,
                    };
                    if side != "any" && side != "left" && side != "right" {
                        c.fail("data.rules: side must be any, left or right");
                    }
                    dr.out = get_int(c, r, "out", -1, 127, Some(-1));
                    dr.offset = get_int(c, r, "offset", -127, 127, Some(0));
                    t.drum_rules.push(dr);
                }
            } else {
                c.fail("data.rules is missing");
            }
            if let Some(chokes) = data.prop("chokes").dyn_obj() {
                for (k, v) in chokes {
                    let from = get_int_value(k);
                    if !v.is_number() || !(0..=127).contains(&from) || v.to_int() < 0 || v.to_int() > 127 {
                        c.fail("data.chokes: notes must be 0..127");
                        continue;
                    }
                    t.dr_chokes.push((from, v.to_int()));
                }
            }
            {
                let mut shank = [-1, -1];
                int_array(c, data, "hatShank", &mut shank);
                t.dr_shank_closed = shank[0];
                t.dr_shank_open = shank[1];
            }
            t.dr_extra_open = num(c, "hatExtraOpen", 0, 127, -1);
            t.dr_foot_splash = num(c, "hatFootSplash", 0, 127, -1);
            t.dr_ride_bell = num(c, "rideBell", 0, 127, -1);
            t.dr_cross_stick = num(c, "crossStick", 0, 127, -1);
            t.dr_pedal_hat = num(c, "pedalHat", 0, 127, -1);
            t.dr_open_shank = num(c, "openHatShank", 0, 127, -1);
            t.dr_splash = num(c, "splash", 0, 127, -1);
            int_array(c, data, "crashes", &mut t.dr_crashes);
            t.dr_aperture_cc = num(c, "hatApertureCC", -1, 127, -1);
            let keys = data.prop("hatCCKeys");
            if keys.dyn_obj().is_some() {
                t.dr_cc_shank = get_int(c, keys, "shank", 0, 127, Some(-1));
                t.dr_cc_tip = get_int(c, keys, "tip", 0, 127, Some(-1));
            }
            t.dr_china = num(c, "china", 0, 127, -1);
            t.dr_ride2 = num(c, "ride2", 0, 127, -1);
            t.dr_bell_cymbal = num(c, "bellCymbal", -1, 127, -1);
        }
        Driver::KeyswitchBass => {
            t.kb_finger = num(c, "finger", 0, 127, t.kb_finger);
            t.kb_pick = num(c, "pick", 0, 127, t.kb_pick);
            t.kb_slap = num(c, "slap", 0, 127, t.kb_slap);
            t.kb_ghost = num(c, "ghost", 0, 127, t.kb_ghost);
            t.kb_palm_cc = num(c, "palmMuteCC", 0, 127, t.kb_palm_cc);
            t.kb_palm_on = num(c, "palmMuteOn", 0, 127, t.kb_palm_on);
            t.kb_pitch_shift = num(c, "writtenPitchShift", -48, 48, t.kb_pitch_shift);
            t.kb_min_note = num(c, "minNote", 0, 127, t.kb_min_note);
            t.kb_hammer = num(c, "hammerKS", 0, 127, -1);
            t.kb_stop = num(c, "stopKS", 0, 127, -1);
            t.kb_down = num(c, "downStrokeKS", 0, 127, -1);
            t.kb_legato_cc = num(c, "legatoCC", 0, 119, -1);
            t.kb_vibrato_cc = num(c, "vibratoCC", 0, 119, -1);
            t.kb_let_ring_cc = num(c, "letRingCC", 0, 119, -1);
            t.kb_pluck_cc = num(c, "pluckCC", -1, 119, -1);
            t.kb_bend_range = num(c, "bendRange", 1, 24, 4);
            t.kb_pos_cc = num(c, "positionCC", -1, 119, -1);
            {
                let mut pk = [-1, -1];
                int_array(c, data, "positionKeys", &mut pk);
                if pk[0] >= 0 && pk[1] > pk[0] {
                    t.kb_pos_lo = pk[0];
                    t.kb_pos_hi = pk[1];
                } else if pk[0] >= 0 {
                    c.fail("data.positionKeys: the first value must be below the second");
                }
            }
            {
                let mut mr = [-1, -1];
                int_array(c, data, "muteRange", &mut mr);
                if mr[0] >= 0 && mr[1] > mr[0] {
                    t.kb_mute_lo = mr[0];
                    t.kb_mute_hi = mr[1];
                } else if mr[0] >= 0 {
                    c.fail("data.muteRange: the first value must be below the second");
                }
            }
        }
        Driver::PianoPedal => {
            int_array(c, data, "halfPedal", &mut t.half_pedal);
            t.soft_pedal_cc = num(c, "softPedalCC", 0, 127, t.soft_pedal_cc);
            t.soft_pedal_max_energy = num(c, "softPedalMaxEnergy", 0, 4, t.soft_pedal_max_energy);
            t.harmonic_pedal_cc = num(c, "harmonicPedalCC", -1, 119, -1);
        }
        Driver::Swam => {
            let fam = data.prop_str("family", "");
            t.swam_family = match fam.as_str() {
                "strings" => 0,
                "brass" => 1,
                "winds" => 2,
                _ => -1,
            };
            if t.swam_family < 0 {
                c.fail("data.family must be strings, brass or winds");
            }
            if let Some(list) = data.prop("instruments").array() {
                for s in list {
                    let name = s.prop_str("name", "");
                    let low = get_int(c, s, "low", 0, 127, None);
                    let high = get_int(c, s, "high", 0, 127, None);
                    t.swam_instruments.push(SwamInstrument { name, low, high });
                }
            } else {
                c.fail("data.instruments is missing");
            }
            t.swam_expr_cc = num(c, "expressionCC", -1, 127, t.swam_expr_cc); // -1: no expression CC
            t.swam_vib_cc = num(c, "vibratoCC", -1, 127, t.swam_vib_cc);
            t.swam_ks_offset = num(c, "keyswitchOffset", 0, 127, t.swam_ks_offset);
            t.swam_keyswitches = get_bool(data, "keyswitches", true);
            {
                let dyn_ = data.prop_str("dynamics", "cc");
                if dyn_ != "cc" && dyn_ != "velocity" {
                    c.fail("data.dynamics must be cc or velocity");
                }
                t.swam_velocity_dynamics = dyn_ == "velocity";
                let vr = data.prop("velocityRange");
                match vr.array() {
                    Some(a) if a.len() == 2 => {
                        t.swam_vel_lo = a[0].to_int().clamp(1, 127);
                        t.swam_vel_hi = a[1].to_int().clamp(t.swam_vel_lo, 127);
                    }
                    _ if !vr.is_void() => c.fail("data.velocityRange must be [low, high]"),
                    _ => {}
                }
                let arts = data.prop("articulations");
                if arts.is_object() {
                    const NAMES: [&str; 5] = ["sustain", "staccato", "pizzicato", "tremolo", "harmonic"];
                    for (name, art) in NAMES.iter().zip(t.swam_arts.iter_mut()) {
                        let a = arts.prop(name);
                        if a.is_void() {
                            continue;
                        }
                        art.key = get_int(c, a, "key", 0, 127, None);
                        let mode = a.prop_str("mode", "latch");
                        if mode != "latch" && mode != "instant" {
                            c.fail(format!("data.articulations.{name}.mode must be latch or instant"));
                        }
                        art.instant = mode == "instant";
                    }
                    t.swam_custom_arts = true;
                } else if !arts.is_void() {
                    c.fail("data.articulations must be an object");
                }
            }
        }
        Driver::ArticulationKeys => {
            let keys = data.prop("keys");
            if !keys.is_object() {
                c.fail("data.keys is missing");
            }
            const SLOT_NAMES: [&str; 4] = ["long", "short", "pluck", "tremolo"];
            for (name, k) in SLOT_NAMES.iter().zip(t.art_keys.iter_mut()) {
                *k = if keys.is_object() { get_int(c, keys, name, 0, 127, Some(-1)) } else { -1 };
            }
            t.art_key_base = num(c, "keyBase", 0, 127, 0);
            t.art_custom_keys = get_bool(data, "customKeys", false);
            t.art_dyn_cc = num(c, "dynamicsCC", -1, 127, -1);
            t.art_expr_cc = num(c, "expressionCC", -1, 127, 11);
            t.art_tremolo = get_bool(data, "tremolo", false);
            t.art_pluck_cap_below_energy = num(c, "pluckCapBelowEnergy", 0, 5, 0);
            t.art_pluck_vel_cap = num(c, "pluckVelCap", 1, 127, 127);
        }
        Driver::Keyswitch => parse_keyswitch(c, data, &mut t),
        Driver::UjamPhrase => {
            int_array(c, data, "phraseKeys", &mut t.uj_phrase_keys);
            {
                // additions: 4 keys; -1 = none (a library without phrase-end additions)
                let ad = data.prop("additions");
                match ad.array() {
                    Some(a) if a.len() == 4 => {
                        for (x, o) in a.iter().zip(t.uj_additions.iter_mut()) {
                            let v = x.to_int();
                            if !x.is_number() || !(-1..=127).contains(&v) {
                                c.fail("data.additions values must be -1..127");
                                break;
                            }
                            *o = v;
                        }
                    }
                    _ if !ad.is_void() => c.fail("data.additions must be 4 numbers"),
                    _ => {}
                }
                let ek = data.prop("endingKeys");
                match ek.array() {
                    Some(a) if (1..=5).contains(&a.len()) => {
                        for (x, o) in a.iter().zip(t.uj_ending_keys.iter_mut()) {
                            let v = x.to_int();
                            if !x.is_number() || !(-1..=127).contains(&v) {
                                c.fail("data.endingKeys values must be -1..127");
                                break;
                            }
                            *o = v;
                        }
                    }
                    _ if !ek.is_void() => c.fail("data.endingKeys must be 1 to 5 numbers"),
                    _ => {}
                }
                match data.prop_str("bassMode", "always").as_str() {
                    "always" => t.uj_bass_mode = 0,
                    "slash" => t.uj_bass_mode = 1,
                    "none" => t.uj_bass_mode = 2,
                    _ => c.fail("data.bassMode must be always, slash or none"),
                }
            }
            {
                let eb = data.prop("energyBase");
                match eb.array() {
                    Some(a) if a.len() == 4 => {
                        for (x, o) in a.iter().zip(t.uj_energy_base.iter_mut()) {
                            *o = x.to_int().clamp(0, 5);
                        }
                    }
                    _ if !eb.is_void() => c.fail("data.energyBase must be 4 numbers 0..5"),
                    _ => {}
                }
            }
            t.uj_stop_key = num(c, "stopKey", -1, 127, t.uj_stop_key);
            t.uj_play_base = num(c, "playBase", 0, 127, t.uj_play_base);
            t.uj_play_high = num(c, "playHigh", 0, 127, t.uj_play_high);
            t.uj_bass_base = num(c, "bassBase", 0, 127, t.uj_bass_base);
            t.uj_mod_cc = num(c, "modCC", -1, 127, t.uj_mod_cc);
        }
        Driver::AmpleGuitar => parse_ample_guitar(c, data, &mut t),
        Driver::Fiddle => {
            let mut range = [55, 91];
            int_array(c, data, "range", &mut range);
            t.fd_low = range[0];
            t.fd_high = range[1];
            if t.fd_low >= t.fd_high {
                c.fail("data.range must be [low, high]");
            }
            t.fd_expr_cc = num(c, "expressionCC", -1, 127, 11);
            let sp = data.prop("splits");
            if sp.is_object() {
                t.fd_split_vib = get_int(c, sp, "vibrato", 1, 127, Some(50));
                t.fd_split_port = get_int(c, sp, "portamento", 1, 127, Some(16));
                t.fd_split_slur = get_int(c, sp, "slur", 1, 127, Some(61));
            } else if !sp.is_void() {
                c.fail("data.splits must be an object");
            }
            let arts = data.prop("articulations");
            const NAMES: [&str; 9] =
                ["tremolo", "scoop", "staccato", "graceUp", "graceDown", "trillUp", "graceSlideUp", "trillDown", "graceSlideDown"];
            if arts.is_object() {
                for (name, k) in NAMES.iter().zip(t.fd_art.iter_mut()) {
                    *k = get_int(c, arts, name, 0, 127, Some(*k));
                }
            } else if !arts.is_void() {
                c.fail("data.articulations must be an object");
            }
            t.fd_portamento_key = num(c, "portamentoKey", -1, 127, 50);
            let ret = data.prop("articulationReturn");
            if ret.is_object() {
                t.fd_return_mode = match ret.prop_str("mode", "unconfirmed").as_str() {
                    "unconfirmed" => 0,
                    "same" => 1,
                    "key" => 2,
                    "hold" => 3,
                    _ => -1,
                };
                if t.fd_return_mode < 0 {
                    c.fail("data.articulationReturn.mode must be unconfirmed, same, key or hold");
                    t.fd_return_mode = 0;
                }
                if t.fd_return_mode == 2 {
                    t.fd_return_key = get_int(c, ret, "key", 0, 127, None);
                }
            } else if !ret.is_void() {
                c.fail("data.articulationReturn must be an object");
            }
            t.fd_bow_seconds = if data.has("bowSeconds") { get_float(c, data, "bowSeconds", 0.5, 20.0) } else { 3.0 };
            for k in t.fd_art {
                if k >= t.fd_low {
                    c.fail("data.articulations keys must lie below data.range");
                }
            }
        }
        Driver::Organ => {
            t.og_expr_cc = num(c, "expressionCC", -1, 119, 11);
            t.og_leslie_cc = num(c, "leslieSpeedCC", -1, 119, 1);
            t.og_perc_cc = num(c, "percussionCC", -1, 119, -1);
            t.og_chorus_cc = num(c, "chorusCC", -1, 119, -1);
            t.og_note_vel = num(c, "noteVelocity", 1, 127, 100);
            int_array(c, data, "drawbarKeys", &mut t.og_drawbar_keys);
            int_array(c, data, "lowerDrawbarKeys", &mut t.og_lower_keys);
            t.og_lower_channel = num(c, "lowerManualChannel", 0, 16, 0);
            {
                let rv = data.prop("registrations");
                const RN: [&str; 4] = ["gospel", "jazz", "rock", "pad"];
                if let Some(ro) = rv.dyn_obj() {
                    for (name, _) in ro {
                        if index_of(&RN, name) < 0 {
                            c.fail(format!("data.registrations.{name} is not gospel, jazz, rock or pad"));
                        }
                    }
                    for (name, reg) in RN.iter().zip(t.og_registrations.iter_mut()) {
                        let a = rv.prop(name);
                        if a.is_void() {
                            continue;
                        }
                        let arr = match a.array() {
                            Some(arr) if arr.len() == 4 => arr,
                            _ => {
                                c.fail(format!("data.registrations.{name} must be 4 numbers"));
                                continue;
                            }
                        };
                        for (x, o) in arr.iter().zip(reg.iter_mut()) {
                            if !x.is_number() || x.to_int() < 0 || x.to_int() > 127 {
                                c.fail(format!("data.registrations.{name} values must be 0..127"));
                                break;
                            }
                            *o = x.to_int();
                        }
                    }
                } else if !rv.is_void() {
                    c.fail("data.registrations must be an object");
                }
            }
            let cv = data.prop("chorusValues");
            const NAMES: [&str; 7] = ["off", "V1", "V2", "V3", "C1", "C2", "C3"];
            if let Some(co) = cv.dyn_obj() {
                for (name, _) in co {
                    if index_of(&NAMES, name) < 0 {
                        c.fail(format!("data.chorusValues.{name} is not off, V1-V3 or C1-C3"));
                    }
                }
                for (name, o) in NAMES.iter().zip(t.og_chorus_values.iter_mut()) {
                    *o = get_int(c, cv, name, 0, 127, Some(*o));
                }
            } else if !cv.is_void() {
                c.fail("data.chorusValues must be an object");
            }
        }
        Driver::Evolution => parse_evolution(c, data, &mut t),
        Driver::IndiginusStrings => parse_indiginus(c, data, &mut t),
        Driver::BrassSection => {
            let mut range = [40, 84];
            int_array(c, data, "range", &mut range);
            t.br_low = range[0];
            t.br_high = range[1];
            if t.br_low >= t.br_high {
                c.fail("data.range must be [low, high]");
            }
            t.br_dyn_cc = num(c, "dynamicsCC", 0, 119, 1);
            t.br_note_vel = num(c, "noteVelocity", 1, 127, 100);
            let sc = data.prop("setupCCs");
            if let Some(so) = sc.dyn_obj() {
                let mut n = 0;
                for (nm, _) in so {
                    let cc = get_int_value(nm);
                    if !contains_only_digits(nm) || cc > 119 || n >= 4 {
                        c.fail(format!("data.setupCCs.{nm} must be a CC number 0..119 (at most 4)"));
                        continue;
                    }
                    let v = get_int(c, sc, nm, 0, 127, None);
                    if let Some(slot) = t.br_setup_ccs.get_mut(n) {
                        *slot = (cc, v);
                    }
                    n += 1;
                }
            } else if !sc.is_void() {
                c.fail("data.setupCCs must be an object");
            }
            let lk = data.prop("legatoKeys");
            if lk.is_object() {
                t.br_legato_off = get_int(c, lk, "off", 0, 127, None);
                t.br_legato_on = get_int(c, lk, "on", 0, 127, None);
            } else if !lk.is_void() {
                c.fail("data.legatoKeys must be an object");
            }
            t.br_instrument_cc = num(c, "instrumentCC", -1, 119, 15);
            let iv = data.prop("instrumentValues");
            if let Some(io) = iv.dyn_obj() {
                for (nm, _) in io {
                    let patch = get_int_value(nm);
                    if !contains_only_digits(nm) || !(1..=26).contains(&patch) {
                        c.fail(format!("data.instrumentValues.{nm} must be a patch number 1..26"));
                        continue;
                    }
                    let v = get_int(c, iv, nm, 0, 127, None);
                    if let Some(slot) = t.br_instrument_value.get_mut(patch as usize) {
                        *slot = v;
                    }
                }
            } else if !iv.is_void() {
                c.fail("data.instrumentValues must be an object");
            }
            let inf = data.prop("instrumentFor");
            if inf.is_object() {
                t.br_patch_ensemble = get_int(c, inf, "ensemble", 1, 26, Some(21));
                t.br_patch_melodic = get_int(c, inf, "melodic", 1, 26, Some(7));
                t.br_patch_low = get_int(c, inf, "low", 1, 26, Some(22));
            } else if !inf.is_void() {
                c.fail("data.instrumentFor must be an object");
            }
        }
    }
    if !c.ok {
        return None;
    }
    Some(t)
}

fn parse_keyswitch(c: &mut Ctx, data: &Var, t: &mut TargetData) {
    let num = |c: &mut Ctx, key: &str, lo: i32, hi: i32, def: i32| get_int(c, data, key, lo, hi, Some(def));
    // Named articulations in file order, then intent -> articulation by name.
    let mut art_names: Vec<String> = Vec::new();
    let arts = data.prop("articulations");
    if let Some(ao) = arts.dyn_obj() {
        for (name, value) in ao {
            let place = format!("data.articulations.{name}");
            let Some(ka) = usize::try_from(t.ks_art_count).ok().and_then(|i| t.ks_arts.get_mut(i)) else {
                c.fail("data.articulations: at most 16");
                break;
            };
            if !value.is_object() {
                c.fail(format!("{place} must be an object"));
                continue;
            }
            let k = value.prop("key");
            if !is_note(k) {
                c.fail(format!("{place}.key must be 0..127"));
            } else {
                ka.key = k.to_int();
            }
            match value.prop_str("mode", "tap").as_str() {
                "tap" => ka.mode = KS_TAP,
                "hold" => ka.mode = KS_HOLD,
                "latch" => ka.mode = KS_LATCH,
                "same" => ka.mode = KS_SAME,
                _ => c.fail(format!("{place}.mode must be tap, hold, latch or same")),
            }
            let zero = Var::Int(0);
            let ml = value.prop_or("minLen", &zero);
            if !ml.is_number() || ml.to_double() < 0.0 || ml.to_double() > 2000.0 {
                c.fail(format!("{place}.minLen must be 0..2000 (ms)"));
            } else {
                ka.min_len_ms = ml.to_double() as f32;
            }
            art_names.push(name.clone());
            if let Some(n) = t.ks_art_names.get_mut(t.ks_art_count as usize) {
                *n = name.clone();
            }
            t.ks_art_count += 1;
        }
    } else {
        c.fail("data.articulations must be an object");
    }
    let use_ = data.prop("use");
    if let Some(uo) = use_.dyn_obj() {
        const INTENTS: [&str; KS_INTENT_COUNT] = ["long", "short", "pluck", "tremolo", "legatoUp", "slideIn", "ghost"];
        for (name, value) in uo {
            let i = index_of(&INTENTS, name);
            if i < 0 {
                c.fail(format!("data.use: unknown intent \"{name}\""));
                continue;
            }
            let v = value.to_jstring();
            let Some(at) = art_names.iter().position(|n| *n == v) else {
                c.fail(format!("data.use.{name}: \"{v}\" is not an articulation"));
                continue;
            };
            if let Some(u) = t.ks_use.get_mut(i as usize) {
                *u = at as i32;
            }
        }
    } else if !use_.is_void() {
        c.fail("data.use must be an object");
    }
    let def = data.prop("default");
    if !def.is_void() {
        let v = def.to_jstring();
        match art_names.iter().position(|n| *n == v) {
            None => c.fail(format!("data.default: \"{v}\" is not an articulation")),
            Some(at) => t.ks_default = at as i32,
        }
    }
    let ms_field = |c: &mut Ctx, obj: &Var, key: &str, lo: f32, hi: f32, def: f32| -> f32 {
        let v = obj.prop(key);
        if v.is_void() {
            return def;
        }
        let d = v.to_double();
        if !v.is_number() || !d.is_finite() || d < f64::from(lo) || d > f64::from(hi) {
            c.fail(format!("data.{key} must be {}..{}", fmt_g(f64::from(lo)), fmt_g(f64::from(hi))));
            return def;
        }
        d as f32
    };
    let lead = data.prop("ccLeadMs");
    if lead.is_object() {
        t.ks_dyn_lead_ms = ms_field(c, lead, "dynamics", 0.0, 100.0, 15.0);
        t.ks_expr_lead_ms = ms_field(c, lead, "expression", 0.0, 100.0, 10.0);
    } else if !lead.is_void() {
        c.fail("data.ccLeadMs must be an object {dynamics, expression}");
    }
    t.ks_key_lead_ms = ms_field(c, data, "keyLeadMs", 0.0, 40.0, 10.0);
    {
        match data.prop_str("return", "none").as_str() {
            "none" => t.ks_return = 0,
            "same" => t.ks_return = 1,
            "key" => t.ks_return = 2,
            _ => c.fail("data.return must be none, same or key"),
        }
        t.ks_return_key = num(c, "returnKey", -1, 127, -1);
        if t.ks_return == 2 && t.ks_return_key < 0 {
            c.fail("data.returnKey is required when return is key");
        }
    }
    t.ks_low = num(c, "low", 0, 127, 0);
    t.ks_high = num(c, "high", 0, 127, 127);
    if t.ks_high - t.ks_low < 11 {
        c.fail("data.low..high must span at least an octave");
    }
    if data.has("low") || data.has("high") {
        let in_range = |k: i32| k >= t.ks_low && k <= t.ks_high;
        let mut clash = t.ks_return_key >= 0 && in_range(t.ks_return_key);
        for a in t.ks_arts.iter().take(t.ks_art_count.max(0) as usize) {
            if a.key >= 0 && in_range(t.ks_key_base + a.key) {
                clash = true;
            }
        }
        if clash {
            c.fail("data.low..high must not contain an articulation or return key");
        }
    }
    let vr = data.prop("velocityRange");
    if !vr.is_void() {
        match vr.array() {
            Some(a)
                if a.len() == 2
                    && a[0].is_number()
                    && a[1].is_number()
                    && a[0].to_int() >= 1
                    && a[1].to_int() <= 127
                    && a[0].to_int() <= a[1].to_int() =>
            {
                t.ks_vel_lo = a[0].to_int();
                t.ks_vel_hi = a[1].to_int();
            }
            _ => c.fail("data.velocityRange must be [low, high], 1..127"),
        }
    }
    t.ks_vibrato_cc = num(c, "vibratoCC", -1, 127, -1);
    t.ks_vibrato_depth = ms_field(c, data, "vibratoDepth", 0.0, 1.0, 0.5);
    let key_list = |c: &mut Ctx, key: &str, out: &mut [i32; 8]| {
        let v = data.prop(key);
        if v.is_void() {
            return;
        }
        let a = match v.array() {
            Some(a) if a.len() <= 8 => a,
            _ => {
                c.fail(format!("data.{key} must be up to 8 keys"));
                return;
            }
        };
        for (k, o) in a.iter().zip(out.iter_mut()) {
            if !is_note(k) {
                c.fail(format!("data.{key} values must be 0..127"));
                return;
            }
            *o = k.to_int();
        }
    };
    key_list(c, "init", &mut t.ks_init);
    key_list(c, "never", &mut t.ks_never);
    if data.has("low") || data.has("high") {
        for k in t.ks_init {
            if k >= t.ks_low && k <= t.ks_high {
                c.fail("data.low..high must not contain an init key");
                break;
            }
        }
    }
    t.ks_key_base = num(c, "keyBase", 0, 127, 0);
    t.ks_dyn_cc = num(c, "dynamicsCC", -1, 127, -1);
    t.ks_expr_cc = num(c, "expressionCC", -1, 127, -1);
    let reset = data.prop("reset");
    if reset.is_object() {
        t.ks_reset_dyn = get_int(c, reset, "dynamics", -1, 127, Some(-1));
        t.ks_reset_expr = get_int(c, reset, "expression", -1, 127, Some(-1));
    } else if !reset.is_void() {
        c.fail("data.reset must be an object");
    }
    t.ks_send_on_change_only = get_bool(data, "sendOnChangeOnly", true);
    t.ks_cap_below_energy = num(c, "pluckCapBelowEnergy", 0, 5, 0);
    t.ks_cap_vel = num(c, "pluckVelCap", 1, 127, 127);
}

fn parse_ample_guitar(c: &mut Ctx, data: &Var, t: &mut TargetData) {
    // Every key is required: a different Ample guitar is a new file with its own numbers.
    let num = |c: &mut Ctx, key: &str, lo: i32, hi: i32, def: i32| get_int(c, data, key, lo, hi, Some(def));
    let key = |c: &mut Ctx, obj: &Var, k: &str, place: &str| -> i32 {
        let v = obj.prop(k);
        if !is_note(v) {
            c.fail(format!("data.{place}{k} must be a note 0..127"));
            return 0;
        }
        v.to_int()
    };
    let keys = |c: &mut Ctx, k: &str, out: &mut [i32]| {
        let a = match data.prop(k).array() {
            Some(a) if a.len() == out.len() => a,
            _ => {
                c.fail(format!("data.{k} must be {} notes", out.len()));
                return;
            }
        };
        for (v, o) in a.iter().zip(out.iter_mut()) {
            if !is_note(v) {
                c.fail(format!("data.{k} values must be notes 0..127"));
                return;
            }
            *o = v.to_int();
        }
    };
    let mut range = [0, 0];
    keys(c, "noteRange", &mut range);
    t.am_note_low = range[0];
    t.am_note_high = range[1];
    if t.am_note_low > t.am_note_high {
        c.fail("data.noteRange must be low, high");
    }
    let mut nv = [0, 0];
    keys(c, "noteVelocity", &mut nv);
    if nv[0] < 1 || nv[0] > nv[1] || nv[1] > 126 {
        c.fail("data.noteVelocity must be low, high (1..126: 127 is a Pop)");
    }
    t.am_note_vel_low = nv[0];
    t.am_note_vel_high = nv[1];
    let arts = data.prop("articulations");
    const ART_NAMES: [&str; 6] = ["sustain", "harmonic", "palmMute", "slideIn", "legatoSlide", "hammerOn"];
    for (name, o) in ART_NAMES.iter().zip(t.am_art.iter_mut()) {
        *o = key(c, arts, name, "articulations.");
    }
    keys(c, "openStrings", &mut t.am_open_strings);
    let mk = data.prop("modeKeys");
    t.am_strum_mode_key = key(c, mk, "strumMode", "modeKeys.");
    t.am_chord_mode_key = key(c, mk, "chordDetect", "modeKeys.");
    t.am_position_key = key(c, mk, "positionMode", "modeKeys.");
    t.am_open_first_key = key(c, mk, "openStringsFirst", "modeKeys.");
    t.am_on_vel = num(c, "onVelocity", 64, 127, 100);
    t.am_off_vel = num(c, "offVelocity", 1, 63, 20);
    let mut cr = [0, 0];
    keys(c, "chordRange", &mut cr);
    t.am_chord_low = cr[0];
    t.am_chord_high = cr[1];
    t.am_root_high = key(c, data, "rootHigh", "");
    if !(t.am_chord_low <= t.am_root_high && t.am_root_high + 11 <= t.am_chord_high) {
        c.fail("data.chordRange must hold rootHigh and the octave above it");
    }
    keys(c, "strings", &mut t.am_strings);
    let st = data.prop("strokes");
    const STROKE_NAMES: [&str; 4] = ["down", "up", "downMuted", "upMuted"];
    for (name, out) in STROKE_NAMES.iter().zip(t.am_strokes.iter_mut()) {
        let a = match st.prop(name).array() {
            Some(a) if a.len() == 2 => a,
            _ => {
                c.fail(format!("data.strokes.{name} must be 2 keys: full, treble"));
                continue;
            }
        };
        for (k, o) in a.iter().zip(out.iter_mut()) {
            if !is_note(k) {
                c.fail(format!("data.strokes.{name} keys must be notes 0..127"));
                break;
            }
            *o = k.to_int();
        }
    }
    let mut sv = [0, 0];
    keys(c, "strokeVelocity", &mut sv);
    if sv[0] < 1 || sv[0] > sv[1] {
        c.fail("data.strokeVelocity must be low, high (1..127)");
    }
    t.am_stroke_vel_low = sv[0];
    t.am_stroke_vel_high = sv[1];
    t.am_strum_time_key = num(c, "strumTimeKey", -1, 127, -1);
    {
        let mut st = [118, 126];
        if data.has("strumTimeVelocity") {
            keys(c, "strumTimeVelocity", &mut st);
        }
        if st[0] > st[1] {
            c.fail("data.strumTimeVelocity must be lo, hi (0..127, lo <= hi)");
        }
        t.am_strum_time_lo = st[0];
        t.am_strum_time_hi = st[1];
    }
    t.am_release_velocity = get_bool(data, "releaseVelocity", false);
    t.am_onset_ms = num(c, "onsetMs", 0, 120, 0);
    keys(c, "muteVoices", &mut t.am_mute_voices);
    let nz = data.prop("strokeNoise");
    let noise_pair = |c: &mut Ctx, k: &str, out: &mut [i32]| {
        let a = match nz.prop(k).array() {
            Some(a) if a.len() == 2 => a,
            _ => {
                c.fail(format!("data.strokeNoise.{k} must be 2 notes"));
                return;
            }
        };
        if !a.iter().all(is_note) {
            c.fail(format!("data.strokeNoise.{k} values must be notes 0..127"));
            return;
        }
        for (v, o) in a.iter().zip(out.iter_mut()) {
            *o = v.to_int();
        }
    };
    noise_pair(c, "down", &mut t.am_noise[0..2]);
    noise_pair(c, "up", &mut t.am_noise[2..4]);
    let fx = data.prop("fx");
    t.am_hit_top = key(c, fx, "hitTop", "fx.");
    t.am_hit_top_muted = key(c, fx, "hitTopMuted", "fx.");
    t.am_strum_mute = key(c, fx, "strumMute", "fx.");
    if fx.has("slap") {
        t.am_slap = key(c, fx, "slap", "fx."); // optional (default 90)
    }
    if fx.has("scratch") {
        t.am_scratch = key(c, fx, "scratch", "fx."); // optional (default 89)
    }
}

fn parse_evolution(c: &mut Ctx, data: &Var, t: &mut TargetData) {
    let num = |c: &mut Ctx, key: &str, lo: i32, hi: i32, def: i32| get_int(c, data, key, lo, hi, Some(def));
    let mut range = [40, 86];
    int_array(c, data, "range", &mut range);
    t.ev_low = range[0];
    t.ev_high = range[1];
    if t.ev_low >= t.ev_high {
        c.fail("data.range must be [low, high]");
    }
    let arts = data.prop("articulations");
    if let Some(ao) = arts.dyn_obj() {
        for (name, _) in ao {
            let i = index_of(&EV_ART_NAMES, name);
            match t.ev_art.get_mut(i.max(0) as usize) {
                Some(o) if i >= 0 => *o = get_int(c, arts, name, 0, 127, None),
                _ => c.fail(format!("data.articulations.{name} is not a known articulation (see README.md)")),
            }
        }
    } else if !arts.is_void() {
        c.fail("data.articulations must be an object");
    }
    let sk = data.prop("stringKeys");
    if !sk.is_void() {
        match sk.array() {
            Some(a) if a.len() <= 6 => {
                for (x, o) in a.iter().zip(t.ev_string_keys.iter_mut()) {
                    if !x.is_number() || x.to_int() < 0 || x.to_int() > 127 {
                        c.fail("data.stringKeys values must be 0..127");
                        break;
                    }
                    *o = x.to_int();
                }
            }
            _ => c.fail("data.stringKeys must be up to 6 numbers"),
        }
    }
    t.ev_string_count = num(c, "stringCount", 1, 6, 6);
    t.ev_drone_key = num(c, "droneKey", -1, 127, -1);
    t.ev_drone_note = num(c, "droneNote", -1, 127, -1);
    t.ev_perf_fx_base = num(c, "perfFxBase", 0, 127, 96);
    let pf = data.prop("perfFx");
    if let Some(po) = pf.dyn_obj() {
        for (name, _) in po {
            let i = index_of(&EV_PERF_FX_NAMES, name);
            match t.ev_perf_fx.get_mut(i.max(0) as usize) {
                Some(o) if i >= 0 => *o = get_int(c, pf, name, 0, 127, None),
                _ => c.fail(format!("data.perfFx.{name} is not a known effect (see README.md)")),
            }
        }
    } else if !pf.is_void() {
        c.fail("data.perfFx must be an object");
    }
    t.ev_vibrato_cc = num(c, "vibratoCC", -1, 119, 1);
    t.ev_legato_st = num(c, "legatoST", 0, 12, 2);
    t.ev_slide_range = num(c, "slideRange", 0, 12, 2);
    t.ev_bypass_legato_key = num(c, "bypassLegatoKey", -1, 127, -1);
    let sk = data.prop("strumKeys");
    if !sk.is_void() {
        match sk.array() {
            Some(a)
                if a.len() == 2
                    && a[0].is_number()
                    && a[1].is_number()
                    && (-1..=127).contains(&a[0].to_int())
                    && (-1..=127).contains(&a[1].to_int()) =>
            {
                t.ev_strum_down = a[0].to_int();
                t.ev_strum_up = a[1].to_int();
            }
            _ => c.fail("data.strumKeys must be [down, up], -1..127"),
        }
    }
    t.ev_strum_engine = get_bool(data, "strumEngine", false);
    if t.ev_strum_engine && (t.ev_strum_down < 0 || t.ev_strum_up < 0) {
        c.fail("data.strumEngine needs data.strumKeys");
    }
    let tk = data.prop("tremoloKeys");
    if tk.is_object() {
        t.ev_trem_in = get_int(c, tk, "in", 0, 127, None);
        t.ev_trem_on = get_int(c, tk, "on", 0, 127, None);
        t.ev_trem_out = get_int(c, tk, "out", 0, 127, None);
    } else if !tk.is_void() {
        c.fail("data.tremoloKeys must be an object");
    }
    let mut nv = [30, 124];
    int_array(c, data, "noteVelocity", &mut nv);
    t.ev_vel_lo = nv[0];
    t.ev_vel_hi = nv[1];
    if t.ev_vel_lo < 1 || t.ev_vel_lo >= t.ev_vel_hi {
        c.fail("data.noteVelocity must be [lo, hi], 1..127");
    }
    t.ev_bend_blues = get_bool(data, "bendBlues", false);
    for k in t.ev_art {
        if k >= t.ev_low {
            c.fail("data.articulations keys must lie below data.range");
        }
    }
    for k in [t.ev_trem_in, t.ev_trem_on, t.ev_trem_out] {
        if k >= t.ev_low {
            c.fail("data.tremoloKeys must lie below data.range");
        }
    }
}

fn parse_indiginus(c: &mut Ctx, data: &Var, t: &mut TargetData) {
    let num = |c: &mut Ctx, key: &str, lo: i32, hi: i32, def: i32| get_int(c, data, key, lo, hi, Some(def));
    let mut range = [55, 88];
    int_array(c, data, "range", &mut range);
    t.in_low = range[0];
    t.in_high = range[1];
    if t.in_low >= t.in_high {
        c.fail("data.range must be [low, high]");
    }
    match data.prop_str("layout", "renegade").as_str() {
        "renegade" => t.in_layout = 0,
        "banjo" => t.in_layout = 1,
        _ => c.fail("data.layout must be renegade or banjo"),
    }
    let arts = data.prop("articulations");
    if let Some(ao) = arts.dyn_obj() {
        for (name, _) in ao {
            let i = index_of(&IN_ART_NAMES, name);
            match t.in_art.get_mut(i.max(0) as usize) {
                Some(o) if i >= 0 => *o = get_int(c, arts, name, 0, 127, None),
                _ => c.fail(format!("data.articulations.{name} is not a known articulation (see README.md)")),
            }
        }
    } else if !arts.is_void() {
        c.fail("data.articulations must be an object");
    }
    let up_to4 = |c: &mut Ctx, key: &str, out: &mut [i32; 4]| {
        let v = data.prop(key);
        if v.is_void() {
            return;
        }
        let a = match v.array() {
            Some(a) if a.len() <= 4 => a,
            _ => {
                c.fail(format!("data.{key} must be up to 4 numbers"));
                return;
            }
        };
        for (x, o) in a.iter().zip(out.iter_mut()) {
            if !x.is_number() || x.to_int() < 0 || x.to_int() > 127 {
                c.fail(format!("data.{key} values must be 0..127"));
                return;
            }
            *o = x.to_int();
        }
    };
    up_to4(c, "neverSend", &mut t.in_never_send);
    up_to4(c, "initKeys", &mut t.in_init_keys);
    t.in_return_key = num(c, "returnKey", -1, 127, -1);
    let ret = data.prop("articulationReturn");
    if ret.is_object() {
        const MODES: [&str; 4] = ["unconfirmed", "hold", "latch", "same"];
        let i = index_of(&MODES, &ret.prop_str("mode", "unconfirmed"));
        if i < 0 {
            c.fail("data.articulationReturn.mode must be unconfirmed, hold, latch or same");
        } else {
            t.in_return_mode = i;
        }
        if t.in_return_mode == 2 && t.in_return_key < 0 {
            c.fail("data.articulationReturn.mode latch needs data.returnKey (a sustain key)");
        }
    } else if !ret.is_void() {
        c.fail("data.articulationReturn must be an object");
    }
    t.in_expr_cc = num(c, "expressionCC", -1, 119, 11);
    t.in_vibrato_cc = num(c, "vibratoCC", -1, 119, -1);
    t.in_master_key_cc = num(c, "masterKeyCC", -1, 119, -1);
    t.in_tremolo_speed_cc = num(c, "tremoloSpeedCC", -1, 119, -1);
    let mc = data.prop("modeCC");
    if mc.is_object() {
        t.in_mode_cc = get_int(c, mc, "cc", 0, 119, None);
    } else if !mc.is_void() {
        c.fail("data.modeCC must be an object");
    }
    let ck = data.prop("chordKeys");
    if ck.is_object() {
        t.in_chord_down = get_int(c, ck, "down", 0, 127, None);
        t.in_chord_up = get_int(c, ck, "up", 0, 127, None);
        t.in_root_select = get_int(c, ck, "rootSelectBase", -1, 127, Some(-1));
        let major = get_int(c, ck, "major", -1, 127, Some(-1));
        let minor = get_int(c, ck, "minor", -1, 127, Some(-1));
        let seventh = get_int(c, ck, "seventh", -1, 127, Some(-1));
        t.in_variation = [major, minor, seventh];
    } else if !ck.is_void() {
        c.fail("data.chordKeys must be an object");
    }
    t.in_slots = get_bool(data, "slots", false);
    if t.in_slots && (t.in_chord_down < 0 || t.in_chord_up < 0) {
        c.fail("data.slots needs data.chordKeys");
    }
    t.in_drone_note = num(c, "droneNote", -1, 127, -1);
    for k in t.in_art {
        if k >= t.in_low {
            c.fail("data.articulations keys must lie below data.range");
        }
    }
    for k in t.in_art {
        for n in t.in_never_send {
            if k >= 0 && k == n {
                c.fail("data.neverSend holds an articulation key");
            }
        }
    }
    t.in_string_count = num(c, "stringCount", 1, 6, if t.in_layout == 1 { 5 } else { 4 });
    indiginus_plucked_view(t);
}

// ---- Loading ------------------------------------------------------------------------------

type SlotOwner = [[Option<String>; K_PLAYER_SLOTS]; K_INSTRUMENTS];

/// Adds a parsed persona to `lib`, applying the first-file-wins conflict rules. True when admitted.
fn admit_persona(
    lib: &mut LibraryData,
    mut p: Box<PersonaData>,
    label: &str,
    errors: &mut Vec<String>,
    slot_owner: &mut SlotOwner,
    seed_owner: &mut [Option<String>],
) -> bool {
    let Some(owner) = usize::try_from(p.seed).ok().and_then(|s| seed_owner.get_mut(s)) else { return false };
    if let Some(o) = owner {
        errors.push(format!("{label}: duplicate seed {} (also {o})", p.seed));
        return false;
    }
    if let Some(q) = lib.personas.iter().find(|q| q.name == p.name) {
        errors.push(format!("{label}: duplicate name \"{}\" (also {})", p.name, q.file));
        return false;
    }
    // A slot number is an ordering hint; 1..16 also fills the Amorph roster, first file first.
    *owner = Some(label.to_string());
    for &(inst, slot) in &p.slots {
        let (Ok(i), Ok(s)) = (usize::try_from(inst), usize::try_from(slot - 1)) else { continue };
        if let (Some(o), Some(r)) = (slot_owner.get_mut(i).and_then(|row| row.get_mut(s)), lib.roster.get_mut(i).and_then(|row| row.get_mut(s)))
            && o.is_none()
        {
            *o = Some(label.to_string());
            *r = p.seed;
        }
    }
    p.file = label.to_string();
    lib.personas.push(*p);
    true
}

/// Never fails. Bad files are skipped and listed in errors. The overlay's players / targets come first and win:
/// a Library file with the same name, seed (or target slot) is left out and listed in `shadowed`.
pub fn load_library(src: &LibrarySource, overlay: &LibraryOverlay) -> Arc<LibraryData> {
    let mut lib = LibraryData::default();
    let mut errors: Vec<String> = Vec::new();
    let mut slot_owner: SlotOwner = Default::default();
    let mut seed_owner: Vec<Option<String>> = vec![None; K_MAX_SEED + 1];

    // The open song's own copies first: they win over Library files with the same name or seed.
    let mut overlay_player_names: Vec<String> = Vec::new();
    let mut overlay_seeds: Vec<i32> = Vec::new();
    for text in &overlay.players {
        let Ok(j) = parse(text) else { continue };
        let label = "(song)";
        let mut c = Ctx { label, errors: &mut errors, ok: true };
        if let Some(p) = parse_persona(&mut c, &j) {
            let (name, seed) = (p.name.clone(), p.seed);
            if admit_persona(&mut lib, p, label, &mut errors, &mut slot_owner, &mut seed_owner) {
                overlay_player_names.push(name.clone());
                overlay_seeds.push(seed);
                lib.overlay_names.push(name);
            }
        }
    }
    for label in src.unreadable.iter().filter(|l| l.starts_with("Players/")) {
        errors.push(format!("{label}: could not be read"));
    }
    for (name, bytes) in src.json_files("Players") {
        let label = format!("Players/{name}");
        let Some(j) = read_file(&label, bytes, &mut errors) else { continue };
        let mut c = Ctx { label: &label, errors: &mut errors, ok: true };
        if let Some(p) = parse_persona(&mut c, &j) {
            if overlay_player_names.contains(&p.name) || overlay_seeds.contains(&p.seed) {
                lib.shadowed.push(label);
                continue;
            }
            admit_persona(&mut lib, p, &label, &mut errors, &mut slot_owner, &mut seed_owner);
        }
    }
    for text in &overlay.targets {
        let Ok(j) = parse(text) else { continue };
        let label = "(song)";
        let mut c = Ctx { label, errors: &mut errors, ok: true };
        let Some(mut t) = parse_target(&mut c, &j) else { continue };
        let clash = lib.target_store.iter().any(|u| u.slot == t.slot || u.name == t.name);
        if clash || (t.slot == 0 && t.driver != Driver::Generic) {
            continue;
        }
        t.file = label.to_string();
        lib.overlay_names.push(t.name.clone());
        lib.target_store.push(*t);
    }
    let overlay_target_count = lib.target_store.len();

    for label in src.unreadable.iter().filter(|l| l.starts_with("Targets/")) {
        errors.push(format!("{label}: could not be read"));
    }
    for (name, bytes) in src.json_files("Targets") {
        let label = format!("Targets/{name}");
        let Some(j) = read_file(&label, bytes, &mut errors) else { continue };
        let mut c = Ctx { label: &label, errors: &mut errors, ok: true };
        let Some(mut t) = parse_target(&mut c, &j) else { continue };
        let mut clash = false;
        for u in &lib.target_store {
            if u.slot == t.slot {
                errors.push(format!("{label}: target slot {} taken by {}", t.slot, u.file));
                clash = true;
                break;
            }
            if u.name == t.name {
                errors.push(format!("{label}: duplicate target name \"{}\" (also {})", t.name, u.file));
                clash = true;
                break;
            }
        }
        let shadowed_by_overlay =
            lib.target_store.iter().take(overlay_target_count).any(|u| u.file == "(song)" && (u.slot == t.slot || u.name == t.name));
        if shadowed_by_overlay {
            lib.shadowed.push(label);
            continue;
        }
        if clash {
            continue;
        }
        if t.slot == 0 && t.driver != Driver::Generic {
            errors.push(format!("{label}: slot 0 is reserved for a generic target"));
            continue;
        }
        t.file = label;
        lib.target_store.push(*t);
    }

    lib.relink();
    if lib.targets[0].is_none() {
        lib.add_built_in_generic();
    }

    // Every player per instrument in browser order: numbered slots ascending, then A-Z.
    for (i, players) in lib.players.iter_mut().enumerate() {
        let mut list: Vec<(i32, &str, i32)> = Vec::new();
        for p in &lib.personas {
            if let Some(&(_, slot)) = p.slots.iter().find(|(inst, _)| *inst == i as i32) {
                list.push((if slot > 0 { slot } else { i32::MAX }, &p.name, p.seed));
            }
        }
        list.sort_by(|a, b| if a.0 != b.0 { a.0.cmp(&b.0) } else { compare_ignore_case(a.1, b.1) });
        players.extend(list.iter().map(|e| e.2));
    }
    lib.errors = errors;
    Arc::new(lib)
}

/// A Library file's JSON, or `None` with its error pushed ("too large", "not valid JSON (...)").
fn read_file(label: &str, bytes: &[u8], errors: &mut Vec<String>) -> Option<Var> {
    if bytes.len() > MAX_FILE_BYTES {
        errors.push(format!("{label}: too large (over 4 MB)"));
        return None;
    }
    match parse(&text_from_bytes(bytes)) {
        Ok(j) => Some(j),
        Err(m) => {
            errors.push(format!("{label}: not valid JSON ({m})"));
            None
        }
    }
}

/// The factory Library, loaded once.
pub fn embedded_library() -> Arc<LibraryData> {
    static LIB: OnceLock<Arc<LibraryData>> = OnceLock::new();
    LIB.get_or_init(|| load_library(&LibrarySource::embedded(), &LibraryOverlay::default())).clone()
}

/// The factory players' names (`factory.json`): the factory band picks its players among these.
pub fn factory_players() -> Vec<String> {
    factory_player_names().to_vec()
}

/// [`factory_players`], parsed once (every new band's settings read it).
pub fn factory_player_names() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let bytes = embedded::files().iter().find(|(p, _)| *p == "factory.json").map(|(_, b)| *b);
        let j = bytes.map(|b| super::json::parse_or_void(&text_from_bytes(b))).unwrap_or_default();
        j.prop("players").array().map(|a| a.iter().map(Var::to_jstring).collect()).unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_one_player(text: &str) -> (Option<Box<PersonaData>>, Vec<String>) {
        let mut errors = Vec::new();
        let mut c = Ctx { label: "p", errors: &mut errors, ok: true };
        let p = parse(text).ok().and_then(|j| parse_persona(&mut c, &j));
        (p, errors)
    }

    #[test]
    fn persona_messages_are_mss() {
        let (p, e) = load_one_player(r#"{"format":2,"name":"session","seed":2.5,"role":"kazoo","style":"x","slots":[],"tags":7,"feel":[]}"#);
        assert!(p.is_none());
        assert_eq!(
            e,
            [
                "p: format must be 1",
                "p: the name \"Session\" is reserved (it means no player): choose another name",
                "p: seed must be a whole number (got 2.5)",
                "p: role \"kazoo\" is not one of keys, bass, drums, guitar, ensemble, melodic",
                "p: style \"x\" is not a known style (see README.md)",
                "p: slots is empty: the player would never be playable",
                "p: tags must be a list of words",
                "p: swingBase must be a number",
                "p: swingSteps must be a number",
                "p: timingMs must be a number",
                "p: rollMs must be a number",
                "p: jitterMs must be a number",
                "p: velVar must be a number",
                "p: arc must be a number",
                "p: pedalMode must be a number",
                "p: fillRate must be a number",
                "p: fillCount must be a number",
                "p: rhLow must be a number",
                "p: rhHigh must be a number",
                "p: rhAnchor must be a number",
                "p: lhLow must be a number",
                "p: feel.laneMs must be 4 numbers",
                "p: accents must be exactly 16 numbers (got 0)",
                "p: grids is missing",
            ]
        );
    }

    #[test]
    fn keys_table_is_complete() {
        assert_eq!(k_key_count(), 293);
        assert_eq!(target_data_keys(1).count(), 16, "drumMap keys");
        assert!(K_KEYS.iter().all(|k| k.block == "player" || k.block.starts_with("player.") || k.block.starts_with("target.")));
    }

    #[test]
    fn every_embedded_player_round_trips_its_grid_ample_flags() {
        let lib = embedded_library();
        let clay = lib.persona(1).expect("Clay");
        assert_eq!(clay.name, "Clay");
        assert_eq!(clay.file, "Players/Clay.json");
        assert!(lib.personas.iter().all(|p| p.grid.iter().any(|&v| v > 0) || p.only12));
    }
}
