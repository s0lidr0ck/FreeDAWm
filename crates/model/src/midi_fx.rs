//! MIDI effects before a track's instrument: an arpeggiator, a chord, a scale (force to scale),
//! note repeat and a script, in a short list per instrument or MIDI track
//! ([`crate::Track::midi_fx`]). A script's text and its parameters' values are kept on the track
//! ([`crate::Track::midi_scripts`], [`MidiScript`]), named by the effect's [`MidiFx::script`].
//!
//! The list is stored with the session; the mix runs it on the audio thread on everything the
//! instrument is given (clips, live input, routed MIDI tracks, the Band), in list order, before
//! the instrument hears it. Every value here is plain and `Copy`, so the mix copies a changed
//! list into fixed storage without allocating. Values out of range (a hand-edited session) are
//! clamped where they are read ([`MidiFx::sane`]).

use serde::{Deserialize, Serialize};

/// Most MIDI effects on one track.
pub const MAX_MIDI_FX: usize = 4;

/// What a MIDI effect does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiFxKind {
    /// Plays the keys held one after another, on the tempo.
    Arpeggiator,
    /// One key plays a chord shape built on it.
    Chord,
    /// Notes outside a scale are moved to the nearest note in it, or left out.
    Scale,
    /// Each key held is played again and again on the tempo.
    NoteRepeat,
    /// A script the user writes ([`MidiScript`]): it passes, changes, leaves out or adds events.
    Script,
}

impl MidiFxKind {
    pub const ALL: [MidiFxKind; 5] = [MidiFxKind::Arpeggiator, MidiFxKind::Chord, MidiFxKind::Scale, MidiFxKind::NoteRepeat, MidiFxKind::Script];

    pub fn name(self) -> &'static str {
        match self {
            MidiFxKind::Arpeggiator => "Arpeggiator",
            MidiFxKind::Chord => "Chord",
            MidiFxKind::Scale => "Scale",
            MidiFxKind::NoteRepeat => "Note Repeat",
            MidiFxKind::Script => "Script",
        }
    }

    /// The command and session spelling (`arpeggiator`, `chord`, `scale`, `note_repeat`, `script`).
    pub fn key(self) -> &'static str {
        match self {
            MidiFxKind::Arpeggiator => "arpeggiator",
            MidiFxKind::Chord => "chord",
            MidiFxKind::Scale => "scale",
            MidiFxKind::NoteRepeat => "note_repeat",
            MidiFxKind::Script => "script",
        }
    }

    pub fn from_key(k: &str) -> Option<MidiFxKind> {
        MidiFxKind::ALL.into_iter().find(|x| x.key() == k)
    }
}

/// The arpeggiator's and note repeat's step lengths: (name, quarter notes).
pub const MIDI_FX_RATES: [(&str, f64); 7] =
    [("1/4", 1.0), ("1/8", 0.5), ("1/8T", 1.0 / 3.0), ("1/16", 0.25), ("1/16T", 1.0 / 6.0), ("1/32", 0.125), ("1/32T", 1.0 / 12.0)];

/// The arpeggiator's orders.
pub const ARP_PATTERNS: [&str; 5] = ["Up", "Down", "Up/Down", "As Played", "Random"];

/// The chord shapes: (name, semitones above the key played; the key itself first).
pub const CHORD_SHAPES: [(&str, &[i8]); 12] = [
    ("Major", &[0, 4, 7]),
    ("Minor", &[0, 3, 7]),
    ("7", &[0, 4, 7, 10]),
    ("Maj7", &[0, 4, 7, 11]),
    ("m7", &[0, 3, 7, 10]),
    ("Sus2", &[0, 2, 7]),
    ("Sus4", &[0, 5, 7]),
    ("Dim", &[0, 3, 6]),
    ("Aug", &[0, 4, 8]),
    ("Power", &[0, 7]),
    ("Power + Octave", &[0, 7, 12]),
    ("Octaves", &[0, 12]),
];

/// The scales: (name, bit per semitone above the root that is in it).
pub const SCALES: [(&str, u16); 12] = [
    ("Major", 0b1010_1011_0101),
    ("Natural Minor", 0b0101_1010_1101),
    ("Harmonic Minor", 0b1001_1010_1101),
    ("Melodic Minor", 0b1010_1010_1101),
    ("Dorian", 0b0110_1010_1101),
    ("Phrygian", 0b0101_1010_1011),
    ("Lydian", 0b1010_1101_0101),
    ("Mixolydian", 0b0110_1011_0101),
    ("Major Pentatonic", 0b0010_1001_0101),
    ("Minor Pentatonic", 0b0100_1010_1001),
    ("Blues", 0b0100_1110_1001),
    ("Chromatic", 0b1111_1111_1111),
];

/// The note names of a scale's root.
pub const ROOT_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// One MIDI effect and its settings. Each kind reads its own fields and keeps the others, so a
/// setting survives a change of kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MidiFx {
    pub kind: MidiFxKind,
    /// Off: events pass through untouched (the notes it started are let go).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bypass: bool,
    /// Arpeggiator, note repeat: the step, an index into [`MIDI_FX_RATES`].
    #[serde(default = "default_rate")]
    pub rate: u8,
    /// Arpeggiator: the order, an index into [`ARP_PATTERNS`].
    #[serde(default)]
    pub pattern: u8,
    /// Arpeggiator: octaves it climbs through (1-4).
    #[serde(default = "one")]
    pub octaves: u8,
    /// Arpeggiator, note repeat: how long each note sounds, percent of the step (5-100).
    #[serde(default = "default_gate")]
    pub gate: u8,
    /// Arpeggiator: keeps playing the last keys after they are let go, until new ones are pressed.
    #[serde(default)]
    pub latch: bool,
    /// Chord: the shape, an index into [`CHORD_SHAPES`].
    #[serde(default)]
    pub chord: u8,
    /// Scale: its root (0 = C … 11 = B).
    #[serde(default)]
    pub root: u8,
    /// Scale: an index into [`SCALES`].
    #[serde(default)]
    pub scale: u8,
    /// Scale: notes outside it are left out instead of moved to the nearest note in it.
    #[serde(default)]
    pub drop: bool,
    /// Scale: follows the chord lane instead of its own root and scale, where the lane has a
    /// chord ([`FOLLOW_MODES`]: 0 no, 1 the chord's tones, 2 the chord's scale); elsewhere its
    /// own scale holds.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub follow: u8,
    /// Script: the [`MidiScript::id`] of its text on the track (0: none yet; it passes events).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub script: u32,
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

/// Most parameters a script declares.
pub const MAX_SCRIPT_PARAMS: usize = 8;

/// Longest script text kept, in bytes.
pub const MAX_SCRIPT_LEN: usize = 64 * 1024;

/// A Script MIDI effect's text and settings, kept on its track ([`crate::Track::midi_scripts`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MidiScript {
    /// Named by [`MidiFx::script`]; unique on the track, from 1.
    pub id: u32,
    /// What the editor calls it (an example's name, or the user's).
    #[serde(default)]
    pub name: String,
    /// The script (Rhai).
    #[serde(default)]
    pub source: String,
    /// The values of the parameters it declares, by name (a parameter not here has its default).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<(String, f64)>,
    /// The compiled script and what went wrong compiling it: made by the engine off the audio
    /// thread, never saved, left out of comparisons.
    #[serde(skip)]
    pub runtime: ScriptRuntime,
}

impl MidiScript {
    /// The value of parameter `name`, if one was set.
    pub fn value(&self, name: &str) -> Option<f64> {
        self.values.iter().find(|(n, _)| n == name).map(|v| v.1)
    }
}

/// Where a [`MidiScript`] keeps its compiled form (the mix's own type, behind `Any`) or the
/// compile error. Derived: two are always equal.
#[derive(Clone, Default)]
pub struct ScriptRuntime {
    pub program: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
    /// A fingerprint of the source the program (or the error) was made from, so a changed text
    /// is compiled again.
    pub made_from: Option<u64>,
    pub error: Option<String>,
}

impl std::fmt::Debug for ScriptRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptRuntime").field("compiled", &self.program.is_some()).field("error", &self.error).finish()
    }
}

impl PartialEq for ScriptRuntime {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

/// What a Scale effect follows on the chord lane: nothing, the chord's tones only, or a scale
/// built on the chord (its tones and the steps between them).
pub const FOLLOW_MODES: [&str; 3] = ["Own scale", "Chord tones", "Chord scale"];

fn default_rate() -> u8 {
    3
}
fn one() -> u8 {
    1
}
fn default_gate() -> u8 {
    50
}

impl MidiFx {
    /// A new effect of `kind` with its default settings (1/16 steps, Up, one octave, half gate,
    /// a major chord, C major).
    pub fn new(kind: MidiFxKind) -> MidiFx {
        MidiFx {
            kind,
            bypass: false,
            rate: default_rate(),
            pattern: 0,
            octaves: 1,
            gate: default_gate(),
            latch: false,
            chord: 0,
            root: 0,
            scale: 0,
            drop: false,
            follow: 0,
            script: 0,
        }
    }

    /// Every value brought into its range.
    pub fn sane(mut self) -> MidiFx {
        self.rate = self.rate.min(MIDI_FX_RATES.len() as u8 - 1);
        self.pattern = self.pattern.min(ARP_PATTERNS.len() as u8 - 1);
        self.octaves = self.octaves.clamp(1, 4);
        self.gate = self.gate.clamp(5, 100);
        self.chord = self.chord.min(CHORD_SHAPES.len() as u8 - 1);
        self.root = self.root.min(11);
        self.scale = self.scale.min(SCALES.len() as u8 - 1);
        self.follow = self.follow.min(FOLLOW_MODES.len() as u8 - 1);
        self
    }

    /// The step length in quarter notes.
    pub fn step_quarters(&self) -> f64 {
        MIDI_FX_RATES.get(usize::from(self.rate)).map_or(0.25, |r| r.1)
    }

    /// A short description of the settings, for menus.
    pub fn summary(&self) -> String {
        let s = self.sane();
        let rate = MIDI_FX_RATES.get(usize::from(s.rate)).map_or("", |r| r.0);
        match s.kind {
            MidiFxKind::Arpeggiator => {
                let pat = ARP_PATTERNS.get(usize::from(s.pattern)).copied().unwrap_or("");
                format!("{pat} {rate}, {} oct{}", s.octaves, if s.latch { ", latch" } else { "" })
            }
            MidiFxKind::Chord => CHORD_SHAPES.get(usize::from(s.chord)).map_or("", |c| c.0).to_string(),
            MidiFxKind::Scale if s.follow > 0 => {
                format!("{} on the chord lane{}", FOLLOW_MODES.get(usize::from(s.follow)).copied().unwrap_or(""), if s.drop { ", drop" } else { "" })
            }
            MidiFxKind::Scale => format!(
                "{} {}{}",
                ROOT_NAMES.get(usize::from(s.root)).copied().unwrap_or(""),
                SCALES.get(usize::from(s.scale)).map_or("", |c| c.0),
                if s.drop { ", drop" } else { "" }
            ),
            MidiFxKind::NoteRepeat => rate.to_string(),
            MidiFxKind::Script => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_have_their_root_and_the_right_number_of_notes() {
        let counts = [7, 7, 7, 7, 7, 7, 7, 7, 5, 5, 6, 12];
        for ((name, bits), n) in SCALES.iter().zip(counts) {
            assert!(bits & 1 != 0, "{name} has its root");
            assert_eq!(bits.count_ones(), n, "{name}");
        }
        // C major: C D E F G A B.
        let major: Vec<u8> = (0..12).filter(|k| SCALES[0].1 & (1 << k) != 0).collect();
        assert_eq!(major, vec![0, 2, 4, 5, 7, 9, 11]);
    }

    #[test]
    fn saved_settings_round_trip_and_bad_values_are_clamped() {
        let mut fx = MidiFx::new(MidiFxKind::Arpeggiator);
        fx.latch = true;
        fx.pattern = 2;
        let text = serde_json::to_string(&fx).unwrap();
        assert_eq!(serde_json::from_str::<MidiFx>(&text).unwrap(), fx);
        let bare: MidiFx = serde_json::from_str(r#"{"kind":"note_repeat"}"#).unwrap();
        assert_eq!(bare, MidiFx::new(MidiFxKind::NoteRepeat));
        let wild = MidiFx { rate: 200, octaves: 0, gate: 0, chord: 99, root: 40, scale: 77, pattern: 9, ..MidiFx::new(MidiFxKind::Chord) }.sane();
        assert_eq!((wild.rate, wild.octaves, wild.gate, wild.chord, wild.root, wild.scale, wild.pattern), (6, 1, 5, 11, 11, 11, 4));
        assert_eq!(MidiFxKind::from_key("note_repeat"), Some(MidiFxKind::NoteRepeat));
    }
}
