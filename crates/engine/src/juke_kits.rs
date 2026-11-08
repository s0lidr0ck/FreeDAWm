//! Juke's starter kits: original by construction, nothing recorded or copied.
//!
//! Each kit is a set of changes to Juke's defaults (voices, tuning, envelopes, sends), a name and
//! family for each pad, and the effects on its buses. The **rendered** kits go one step further:
//! when one loads, every pad's synth voice is rendered here, at four velocities (and, for the hats,
//! snares and claps, two round-robin takes a few cents apart), into samples the pads then play.
//! The samples become session audio like any loaded sample, so the kit travels with the project.

use soundcraft_audio_io::AudioBuffer;
use soundcraft_dsp::plugins::juke::{JUKE_INFO, LAYERS, PADS};
use soundcraft_model::{DrumFamily, KitBus};

/// A starter kit.
pub struct Kit {
    pub name: &'static str,
    /// Its pads play samples rendered from their synth voices when it loads.
    pub rendered: bool,
    /// Pad names and families, in pad order; a pad not listed keeps the default kit's.
    pub pads: &'static [(&'static str, DrumFamily)],
    /// Parameter changes from Juke's defaults (`p3_tune`, `return_a`, …).
    pub params: &'static [(&'static str, f32)],
    /// Effects on the buses: (bus, slot from 0, plugin, its parameter changes).
    pub fx: &'static [(KitBus, u8, &'static str, &'static [(&'static str, f32)])],
}

use DrumFamily::{Clap, Hat, Kick, Percussion, Snare, Tom};

/// A change a [`Character`] makes to a pad parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    Set(f32),
    Add(f32),
    Times(f32),
}

/// A character for a kit: settings and bus effects laid over it when it loads, so one recording
/// gives several kits (`mix.kit_load {…, character}`).
#[derive(Debug, Clone, Copy)]
pub struct Character {
    pub name: &'static str,
    pub about: &'static str,
    /// (the pad families it touches, every pad when empty; a pad parameter without its `pN_`;
    /// the change).
    pub pads: &'static [(&'static [DrumFamily], &'static str, Op)],
    /// Kit-wide parameters (`level`, `return_a`, `humanize`…).
    pub kit: &'static [(&'static str, f32)],
    /// Effects on the buses: (bus, slot from 0, plugin, its parameter changes).
    pub fx: &'static [(KitBus, u8, &'static str, &'static [(&'static str, f32)])],
}

const DRUMS: &[DrumFamily] = &[Kick, Snare, Clap, Tom, Percussion];
const SHELLS: &[DrumFamily] = &[Kick, Snare, Tom];

/// The characters, for any kit and above all the recorded ones.
pub static CHARACTERS: &[Character] = &[
    Character {
        name: "Tight",
        about: "short, dry and compressed: every hit stops quickly",
        pads: &[(DRUMS, "decay", Op::Set(320.0)), (&[Hat], "decay", Op::Set(260.0)), (&[DrumFamily::Cymbal], "decay", Op::Set(1400.0))],
        kit: &[],
        fx: &[(KitBus::Out, 0, "fs_comp", &[])],
    },
    Character {
        name: "Punch",
        about: "drums driven a little, transients and compression on the output",
        pads: &[(SHELLS, "drive", Op::Set(18.0)), (&[Kick], "level", Op::Add(1.5))],
        kit: &[],
        fx: &[(KitBus::Out, 0, "fs_transient", &[]), (KitBus::Out, 1, "fs_comp", &[])],
    },
    Character {
        name: "Big Room",
        about: "every pad sent to a chamber on bus A",
        pads: &[(&[], "send_a", Op::Set(35.0)), (&[Kick], "send_a", Op::Set(15.0))],
        kit: &[("return_a", -3.0)],
        fx: &[(KitBus::A, 0, "fs_chamber", &[])],
    },
    Character {
        name: "Vintage",
        about: "tuned down a little, the top rolled off, through tape",
        pads: &[(&[], "tune", Op::Add(-1.0)), (&[], "filter", Op::Set(1.0)), (&[], "cutoff", Op::Set(7500.0))],
        kit: &[],
        fx: &[(KitBus::Out, 0, "fs_tape", &[])],
    },
    Character {
        name: "Lo-fi",
        about: "dark, crunchy and narrow, like a sampled record",
        pads: &[(&[], "filter", Op::Set(1.0)), (&[], "cutoff", Op::Set(3800.0)), (&[], "drive", Op::Set(30.0)), (&[], "pan", Op::Times(0.4))],
        kit: &[],
        fx: &[(KitBus::Out, 0, "lofi", &[]), (KitBus::Out, 1, "fs_comp", &[])],
    },
    Character {
        name: "Tuned Up",
        about: "shells tuned up, short toms and a small room: a jazz or funk kit",
        pads: &[(SHELLS, "tune", Op::Add(3.0)), (&[Tom], "decay", Op::Set(700.0)), (&[], "send_b", Op::Set(15.0))],
        kit: &[("return_b", -6.0)],
        fx: &[(KitBus::B, 0, "fs_reverb", &[])],
    },
    Character {
        name: "Tuned Down",
        about: "shells tuned down and driven, glued on the output: a heavy kit",
        pads: &[(SHELLS, "tune", Op::Add(-3.0)), (SHELLS, "drive", Op::Set(15.0))],
        kit: &[],
        fx: &[(KitBus::Out, 0, "fs_comp", &[]), (KitBus::Out, 1, "fs_sat", &[])],
    },
    Character {
        name: "Loose",
        about: "humanized: every pad varies in level, pitch and timing",
        pads: &[(&[], "hum_level", Op::Set(1.5)), (&[], "hum_pitch", Op::Set(8.0)), (&[], "hum_timing", Op::Set(4.0))],
        kit: &[("humanize", 20.0)],
        fx: &[],
    },
];

/// The character named `name` (case and spaces ignored).
pub fn character(name: &str) -> Option<&'static Character> {
    let key = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let k = key(name);
    CHARACTERS.iter().find(|c| key(c.name) == k)
}

/// Lays `c`'s settings over `params` (a Juke's parameters); `family` gives each pad's family.
/// Values stay within each parameter's range.
pub fn apply_character(c: &Character, params: &mut std::collections::BTreeMap<String, f32>, family: impl Fn(usize) -> DrumFamily) {
    let info = &soundcraft_dsp::plugins::juke::JUKE_INFO;
    let current = |params: &std::collections::BTreeMap<String, f32>, k: &str| {
        params.get(k).copied().or_else(|| info.param(k).map(|q| q.default)).unwrap_or(0.0)
    };
    for pad in 0..PADS {
        for (families, name, op) in c.pads {
            if !families.is_empty() && !families.contains(&family(pad)) {
                continue;
            }
            let key = format!("p{}_{name}", pad + 1);
            let Some(q) = info.param(&key) else { continue };
            let v = current(params, &key);
            let nv = match op {
                Op::Set(x) => *x,
                Op::Add(x) => v + x,
                Op::Times(x) => v * x,
            };
            params.insert(key, q.clamp(nv));
        }
    }
    for (k, v) in c.kit {
        if let Some(q) = info.param(k) {
            params.insert((*k).to_string(), q.clamp(*v));
        }
    }
}

/// The kits: the five starter kits (the default first), then one per style ([`crate::juke_styles`]).
pub static KITS: &[Kit] = &[
    Kit { name: "Porch Light", rendered: false, pads: &[], params: &[], fx: &[] },
    Kit {
        name: "Tin Roof",
        rendered: true,
        pads: &[
            ("Tin kick", Kick),
            ("Rim", Percussion),
            ("Tin snare", Snare),
            ("Clap", Clap),
            ("Tight hat", Hat),
            ("Foot hat", Hat),
            ("Open hat", Hat),
        ],
        params: &[
            ("p1_tune", 2.0),
            ("p1_decay", 320.0),
            ("p1_pitch_amount", 9.0),
            ("p1_pitch_decay", 35.0),
            ("p3_tone", 68.0),
            ("p3_snappy", 82.0),
            ("p3_decay", 180.0),
            ("p4_decay", 240.0),
            ("p5_decay", 45.0),
            ("p5_tone", 70.0),
            ("p7_decay", 420.0),
            ("humanize", 15.0),
        ],
        fx: &[],
    },
    Kit {
        name: "Muddy Water",
        rendered: false,
        pads: &[
            ("Deep kick", Kick),
            ("Rim", Percussion),
            ("Mud snare", Snare),
            ("Clap", Clap),
            ("Closed hat", Hat),
            ("Pedal hat", Hat),
            ("Open hat", Hat),
            ("Floor tom", Tom),
            ("Low tom", Tom),
            ("Mid tom", Tom),
        ],
        params: &[
            ("p1_tune", -5.0),
            ("p1_decay", 1300.0),
            ("p1_pitch_amount", 12.0),
            ("p1_pitch_decay", 90.0),
            ("p1_drive", 22.0),
            ("p3_tone", 30.0),
            ("p3_decay", 260.0),
            ("p3_send_a", 25.0),
            ("p4_send_a", 30.0),
            ("p8_tune", -9.0),
            ("p9_tune", -5.0),
            ("p10_tune", -1.0),
            ("return_a", -8.0),
        ],
        fx: &[(KitBus::A, 0, "fs_reverb", &[])],
    },
    Kit {
        name: "Back Forty",
        rendered: true,
        pads: &[("Dusty kick", Kick), ("Rim", Percussion), ("Dusty snare", Snare), ("Clap", Clap), ("Closed hat", Hat)],
        params: &[
            ("p1_drive", 35.0),
            ("p1_filter", 1.0),
            ("p1_cutoff", 4000.0),
            ("p3_drive", 30.0),
            ("p3_filter", 1.0),
            ("p3_cutoff", 6000.0),
            ("p4_drive", 25.0),
            ("p5_filter", 2.0),
            ("p5_cutoff", 3000.0),
            ("p6_filter", 2.0),
            ("p6_cutoff", 3000.0),
            ("p7_filter", 2.0),
            ("p7_cutoff", 3000.0),
            ("humanize", 25.0),
        ],
        fx: &[(KitBus::Out, 0, "fs_sat", &[])],
    },
    Kit {
        name: "Revival Tent",
        rendered: true,
        pads: &[("Big kick", Kick), ("Stick", Percussion), ("Tent snare", Snare), ("Handclap", Clap)],
        params: &[
            ("p1_decay", 700.0),
            ("p3_send_a", 35.0),
            ("p4_send_a", 40.0),
            ("p2_send_b", 25.0),
            ("p4_send_b", 15.0),
            ("p8_send_a", 25.0),
            ("p9_send_a", 25.0),
            ("p10_send_a", 25.0),
            ("return_a", -4.0),
            ("return_b", -10.0),
        ],
        fx: &[(KitBus::A, 0, "fs_reverb", &[]), (KitBus::B, 0, "fs_delay", &[])],
    },
    // Style kits (crate::juke_styles), each named after its style.
    Kit {
        name: "Boom Bap",
        rendered: true,
        pads: &[("Dusty kick", Kick), ("Rim", Percussion), ("Crack snare", Snare), ("Clap", Clap), ("Closed hat", Hat)],
        params: &[
            ("p1_tune", -2.0),
            ("p1_decay", 380.0),
            ("p1_drive", 30.0),
            ("p1_pitch_amount", 6.0),
            ("p1_pitch_decay", 40.0),
            ("p3_tone", 40.0),
            ("p3_snappy", 75.0),
            ("p3_decay", 240.0),
            ("p3_drive", 20.0),
            ("p5_filter", 1.0),
            ("p5_cutoff", 7000.0),
            ("p7_filter", 1.0),
            ("p7_cutoff", 7000.0),
            ("humanize", 20.0),
        ],
        fx: &[(KitBus::Out, 0, "fs_sat", &[])],
    },
    Kit {
        name: "Trap",
        rendered: false,
        pads: &[("808", Kick), ("Rim", Percussion), ("Snare", Snare), ("Clap", Clap), ("Hat", Hat), ("Pedal hat", Hat), ("Open hat", Hat)],
        params: &[
            ("p1_tune", -7.0),
            ("p1_decay", 2200.0),
            ("p1_pitch_amount", 12.0),
            ("p1_pitch_decay", 70.0),
            ("p1_drive", 25.0),
            ("p3_tone", 70.0),
            ("p3_snappy", 85.0),
            ("p3_decay", 160.0),
            ("p4_decay", 220.0),
            ("p4_send_a", 25.0),
            ("p5_decay", 40.0),
            ("p5_tone", 80.0),
            ("p7_decay", 350.0),
            ("return_a", -10.0),
        ],
        fx: &[(KitBus::A, 0, "fs_reverb", &[])],
    },
    Kit {
        name: "House",
        rendered: false,
        pads: &[("House kick", Kick), ("Rim", Percussion), ("Snare", Snare), ("Clap", Clap)],
        params: &[
            ("p1_decay", 320.0),
            ("p1_pitch_amount", 10.0),
            ("p1_pitch_decay", 30.0),
            ("p4_decay", 260.0),
            ("p4_send_a", 20.0),
            ("p5_decay", 50.0),
            ("p5_tone", 70.0),
            ("p7_decay", 260.0),
            ("return_a", -12.0),
        ],
        fx: &[(KitBus::A, 0, "fs_reverb", &[])],
    },
    Kit {
        name: "Techno",
        rendered: false,
        pads: &[("Techno kick", Kick), ("Rim", Percussion)],
        params: &[
            ("p1_tune", -3.0),
            ("p1_decay", 400.0),
            ("p1_drive", 40.0),
            ("p2_decay", 50.0),
            ("p2_send_b", 30.0),
            ("p5_decay", 35.0),
            ("p12_decay", 900.0),
            ("return_b", -12.0),
        ],
        fx: &[(KitBus::B, 0, "fs_delay", &[])],
    },
    Kit {
        name: "Lo-fi",
        rendered: true,
        pads: &[("Soft kick", Kick), ("Rim", Percussion), ("Soft snare", Snare)],
        params: &[
            ("p1_filter", 1.0),
            ("p1_cutoff", 4500.0),
            ("p1_drive", 25.0),
            ("p3_filter", 1.0),
            ("p3_cutoff", 4500.0),
            ("p3_tone", 35.0),
            ("p5_filter", 1.0),
            ("p5_cutoff", 4500.0),
            ("p7_filter", 1.0),
            ("p7_cutoff", 4500.0),
            ("humanize", 30.0),
        ],
        fx: &[(KitBus::Out, 0, "fs_tape", &[])],
    },
    Kit {
        name: "Funk",
        rendered: true,
        pads: &[("Tight kick", Kick), ("Rim", Percussion), ("Funk snare", Snare)],
        params: &[
            ("p1_decay", 300.0),
            ("p1_tone", 60.0),
            ("p3_tone", 65.0),
            ("p3_snappy", 70.0),
            ("p3_decay", 180.0),
            ("p5_decay", 55.0),
            ("p7_decay", 500.0),
            ("humanize", 15.0),
        ],
        fx: &[],
    },
    Kit {
        name: "Reggaeton",
        rendered: false,
        pads: &[("Kick", Kick), ("Rim", Percussion), ("Dembow snare", Snare), ("Clap", Clap)],
        params: &[
            ("p1_tune", -4.0),
            ("p1_decay", 600.0),
            ("p1_pitch_amount", 8.0),
            ("p3_tone", 60.0),
            ("p3_decay", 150.0),
            ("p3_send_a", 15.0),
            ("p4_send_a", 15.0),
            ("return_a", -12.0),
        ],
        fx: &[(KitBus::A, 0, "fs_reverb", &[])],
    },
    Kit {
        name: "Drum & Bass",
        rendered: false,
        pads: &[("Break kick", Kick), ("Rim", Percussion), ("Break snare", Snare)],
        params: &[
            ("p1_decay", 250.0),
            ("p1_tone", 60.0),
            ("p1_pitch_amount", 14.0),
            ("p1_pitch_decay", 25.0),
            ("p3_tune", 3.0),
            ("p3_tone", 75.0),
            ("p3_snappy", 80.0),
            ("p3_decay", 200.0),
            ("p3_send_a", 15.0),
            ("p5_decay", 30.0),
            ("return_a", -12.0),
        ],
        fx: &[(KitBus::A, 0, "fs_reverb", &[])],
    },
    Kit {
        name: "Country",
        rendered: true,
        pads: &[("Kick", Kick), ("Rim", Percussion), ("Brush snare", Snare)],
        params: &[
            ("p1_decay", 350.0),
            ("p1_tone", 40.0),
            ("p3_tone", 35.0),
            ("p3_snappy", 40.0),
            ("p3_decay", 260.0),
            ("p5_decay", 60.0),
            ("humanize", 25.0),
        ],
        fx: &[],
    },
    Kit {
        name: "Alternative",
        rendered: true,
        pads: &[("Kick", Kick), ("Rim", Percussion), ("Snare", Snare)],
        params: &[
            ("p1_decay", 400.0),
            ("p1_drive", 15.0),
            ("p3_tone", 55.0),
            ("p3_snappy", 65.0),
            ("p3_decay", 300.0),
            ("p3_send_a", 20.0),
            ("p11_send_a", 15.0),
            ("return_a", -10.0),
            ("humanize", 15.0),
        ],
        fx: &[(KitBus::A, 0, "fs_reverb", &[])],
    },
    Kit {
        name: "Soft Rock",
        rendered: true,
        pads: &[("Kick", Kick), ("Cross-stick", Percussion), ("Snare", Snare)],
        params: &[
            ("p1_decay", 380.0),
            ("p1_tone", 40.0),
            ("p3_tone", 40.0),
            ("p3_snappy", 45.0),
            ("p3_decay", 260.0),
            ("p3_send_a", 25.0),
            ("p5_decay", 60.0),
            ("p5_tone", 40.0),
            ("return_a", -10.0),
            ("humanize", 15.0),
        ],
        fx: &[(KitBus::A, 0, "fs_reverb", &[])],
    },
    Kit {
        name: "Hard Rock",
        rendered: true,
        pads: &[("Big kick", Kick), ("Rim", Percussion), ("Big snare", Snare)],
        params: &[
            ("p1_decay", 380.0),
            ("p1_drive", 30.0),
            ("p1_tone", 60.0),
            ("p3_tone", 60.0),
            ("p3_snappy", 70.0),
            ("p3_decay", 340.0),
            ("p3_drive", 25.0),
            ("p3_send_a", 25.0),
            ("p8_drive", 20.0),
            ("p9_drive", 20.0),
            ("p10_drive", 20.0),
            ("return_a", -8.0),
            ("humanize", 10.0),
        ],
        fx: &[(KitBus::A, 0, "fs_reverb", &[]), (KitBus::Out, 0, "fs_comp", &[])],
    },
    Kit {
        name: "Metal",
        rendered: true,
        pads: &[("Click kick", Kick), ("Rim", Percussion), ("Metal snare", Snare)],
        params: &[
            ("p1_decay", 180.0),
            ("p1_tone", 85.0),
            ("p1_pitch_amount", 6.0),
            ("p1_pitch_decay", 15.0),
            ("p1_drive", 20.0),
            ("p3_tone", 70.0),
            ("p3_snappy", 75.0),
            ("p3_decay", 250.0),
            ("p3_drive", 20.0),
            ("p8_decay", 350.0),
            ("p9_decay", 350.0),
            ("p10_decay", 350.0),
        ],
        fx: &[(KitBus::Out, 0, "fs_comp", &[])],
    },
    Kit {
        name: "'50s Rock",
        rendered: true,
        pads: &[("Kick", Kick), ("Rim", Percussion), ("Snare", Snare)],
        params: &[
            ("p1_decay", 300.0),
            ("p1_tone", 30.0),
            ("p1_level", -3.0),
            ("p3_tone", 35.0),
            ("p3_snappy", 55.0),
            ("p3_decay", 200.0),
            ("p3_send_a", 20.0),
            ("p12_decay", 1200.0),
            ("p12_filter", 1.0),
            ("p12_cutoff", 8000.0),
            ("p11_filter", 1.0),
            ("p11_cutoff", 8000.0),
            ("return_a", -12.0),
            ("humanize", 25.0),
        ],
        fx: &[(KitBus::A, 0, "fs_reverb", &[])],
    },
    Kit {
        name: "'60s Rock",
        rendered: true,
        pads: &[("Kick", Kick), ("Rim", Percussion), ("Snare", Snare)],
        params: &[
            ("p1_decay", 350.0),
            ("p1_tone", 40.0),
            ("p3_tone", 45.0),
            ("p3_snappy", 60.0),
            ("p3_decay", 260.0),
            ("p3_send_a", 15.0),
            ("p5_pan", -60.0),
            ("p6_pan", -60.0),
            ("p7_pan", -60.0),
            ("p8_pan", 50.0),
            ("p9_pan", 30.0),
            ("p10_pan", 10.0),
            ("return_a", -12.0),
            ("humanize", 20.0),
        ],
        fx: &[(KitBus::A, 0, "fs_reverb", &[]), (KitBus::Out, 0, "fs_tape", &[])],
    },
    Kit {
        name: "Rockabilly",
        rendered: true,
        pads: &[("Kick", Kick), ("Rim", Percussion), ("Slap snare", Snare)],
        params: &[
            ("p1_decay", 250.0),
            ("p1_tone", 50.0),
            ("p1_level", -3.0),
            ("p3_tone", 60.0),
            ("p3_snappy", 70.0),
            ("p3_decay", 150.0),
            ("p3_send_b", 30.0),
            ("return_b", -10.0),
            ("humanize", 20.0),
        ],
        fx: &[(KitBus::B, 0, "fs_delay", &[])],
    },
    Kit {
        name: "Pop",
        rendered: true,
        pads: &[("Pop kick", Kick), ("Rim", Percussion), ("Snare", Snare), ("Clap", Clap)],
        params: &[
            ("p1_decay", 400.0),
            ("p1_pitch_amount", 8.0),
            ("p3_tone", 65.0),
            ("p4_send_a", 25.0),
            ("p5_decay", 45.0),
            ("p5_tone", 70.0),
            ("return_a", -10.0),
        ],
        fx: &[(KitBus::A, 0, "fs_reverb", &[]), (KitBus::Out, 0, "fs_comp", &[])],
    },
    Kit {
        name: "Folk",
        rendered: true,
        pads: &[("Soft kick", Kick), ("Rim", Percussion), ("Brush snare", Snare)],
        params: &[
            ("p1_decay", 450.0),
            ("p1_tone", 30.0),
            ("p1_level", -4.0),
            ("p3_tone", 30.0),
            ("p3_snappy", 35.0),
            ("p3_decay", 200.0),
            ("p3_send_a", 15.0),
            ("p13_level", 2.0),
            ("p15_level", 2.0),
            ("return_a", -12.0),
            ("humanize", 30.0),
        ],
        fx: &[(KitBus::A, 0, "fs_reverb", &[])],
    },
    Kit {
        name: "Praise",
        rendered: true,
        pads: &[("Kick", Kick), ("Rim", Percussion), ("Bright snare", Snare), ("Clap", Clap)],
        params: &[
            ("p1_decay", 380.0),
            ("p1_tone", 55.0),
            ("p3_tone", 60.0),
            ("p3_snappy", 65.0),
            ("p3_decay", 240.0),
            ("p3_send_a", 20.0),
            ("p4_send_a", 20.0),
            ("return_a", -10.0),
            ("humanize", 10.0),
        ],
        fx: &[(KitBus::A, 0, "fs_reverb", &[])],
    },
    Kit {
        name: "Worship",
        rendered: true,
        pads: &[("Deep kick", Kick), ("Rim", Percussion), ("Wide snare", Snare), ("Clap", Clap)],
        params: &[
            ("p1_tune", -2.0),
            ("p1_decay", 600.0),
            ("p1_tone", 35.0),
            ("p3_tone", 45.0),
            ("p3_snappy", 50.0),
            ("p3_decay", 360.0),
            ("p3_send_a", 40.0),
            ("p3_send_b", 15.0),
            ("p8_decay", 700.0),
            ("p8_send_a", 30.0),
            ("p9_decay", 650.0),
            ("p9_send_a", 30.0),
            ("p10_decay", 600.0),
            ("p10_send_a", 30.0),
            ("p11_send_a", 25.0),
            ("p12_send_a", 25.0),
            ("return_a", -6.0),
            ("return_b", -14.0),
        ],
        fx: &[(KitBus::A, 0, "fs_reverb", &[]), (KitBus::B, 0, "fs_delay", &[])],
    },
    Kit {
        name: "Gospel / Shout",
        rendered: true,
        pads: &[("Kick", Kick), ("Rim", Percussion), ("Shout snare", Snare), ("Clap", Clap)],
        params: &[
            ("p1_decay", 280.0),
            ("p1_tone", 60.0),
            ("p3_tune", 2.0),
            ("p3_tone", 70.0),
            ("p3_snappy", 80.0),
            ("p3_decay", 180.0),
            ("p5_decay", 40.0),
            ("p5_tone", 65.0),
            ("p8_tune", 2.0),
            ("p9_tune", 2.0),
            ("p10_tune", 2.0),
            ("humanize", 10.0),
        ],
        fx: &[(KitBus::Out, 0, "fs_comp", &[])],
    },
];

/// How many of [`KITS`] are the starter kits (the rest are the styles').
pub const STARTER_KITS: usize = 5;

/// The kit named `name` (any case).
pub fn find(name: &str) -> Option<&'static Kit> {
    KITS.iter().find(|k| k.name.eq_ignore_ascii_case(name.trim()))
}

/// The velocities a rendered pad's four layers are taken at, softest first.
pub const LAYER_VELOCITIES: [u8; LAYERS] = [40, 72, 100, 127];

/// Longest a rendered hit runs, in seconds.
const MAX_SECONDS: f32 = 3.0;

/// Round-robin takes a pad gets: two for the hats, snares and claps (a few cents apart), one
/// for the rest.
pub fn takes(family: DrumFamily) -> usize {
    if matches!(family, Hat | Snare | Clap) { 2 } else { 1 }
}

/// Renders pad `pad` (from 0) of a Juke set to `params` (the full parameter map) at `velocity`,
/// detuned by `cents`: the pad alone, centred, at unity, through no bus and no kit level, its
/// silent tail trimmed. Mono, at `rate`. `None` when nothing sounds.
pub fn render_pad(params: &std::collections::BTreeMap<String, f32>, pad: usize, velocity: u8, cents: f32, rate: u32) -> Option<AudioBuffer> {
    let mut j = soundcraft_dsp::create(JUKE_INFO.id)?;
    j.prepare(rate as f32, 512, 2);
    for (k, v) in params {
        j.set_param(k, *v);
    }
    // This pad alone: every other pad plays samples it has none of.
    for other in (0..PADS).filter(|p| *p != pad) {
        j.set_param(&format!("p{}_source", other + 1), 0.0);
    }
    let n = pad + 1;
    for (k, v) in [
        ("source", 1.0),
        ("pan", 0.0),
        ("level", 0.0),
        ("send_a", 0.0),
        ("send_b", 0.0),
        ("velocity", 100.0),
        ("mute", 0.0),
        ("solo", 0.0),
        ("stack1", 0.0),
        ("stack2", 0.0),
        ("mode", 0.0),
        ("reverse", 0.0),
    ] {
        j.set_param(&format!("p{n}_{k}"), v);
    }
    let fine = params.get(&format!("p{n}_fine")).copied().unwrap_or(0.0);
    j.set_param(&format!("p{n}_fine"), (fine + cents).clamp(-100.0, 100.0));
    for (k, v) in [("level", 0.0), ("humanize", 0.0)] {
        j.set_param(k, v);
    }
    let note = params.get(&format!("p{n}_note")).copied().unwrap_or(36.0).round().clamp(0.0, 127.0) as u8;
    // Let the kit level's glide settle before the hit.
    let mut io = vec![vec![0.0f32; 512]; 2];
    for _ in 0..8 {
        j.process(&mut io, 512);
    }
    j.note_on(0, note, velocity);
    let max = (rate as f32 * MAX_SECONDS) as usize;
    let mut out = Vec::with_capacity(max.min(rate as usize));
    let mut quiet = 0usize;
    while out.len() < max {
        j.process(&mut io, 512);
        // Centred: each side carries the hit at -3 dB.
        out.extend(io[0].iter().map(|x| x * std::f32::consts::SQRT_2));
        let peak = io[0].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        quiet = if peak < 1.0e-4 { quiet + 512 } else { 0 };
        if quiet >= rate as usize / 20 && out.len() > quiet {
            break;
        }
    }
    let end = out.iter().rposition(|x| x.abs() >= 1.0e-4)?;
    out.truncate(end + 1);
    // A short fade so a trimmed tail never clicks.
    let fade = (rate as usize / 200).min(out.len());
    let len = out.len();
    for (i, x) in out.iter_mut().skip(len - fade).enumerate() {
        *x *= 1.0 - i as f32 / fade.max(1) as f32;
    }
    Some(AudioBuffer { sample_rate: rate, channels: vec![out] })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kit_names_real_parameters_and_effects() {
        for k in KITS {
            for (id, v) in k.params {
                let p = JUKE_INFO.param(id).unwrap_or_else(|| panic!("{}: no parameter {id}", k.name));
                assert!((p.min..=p.max).contains(v), "{}: {id} = {v}", k.name);
            }
            assert!(k.pads.len() <= PADS);
            for (_, slot, plugin, params) in k.fx {
                let info = soundcraft_dsp::plugin_info(plugin).unwrap_or_else(|| panic!("{}: no plugin {plugin}", k.name));
                assert!(!info.is_instrument && usize::from(*slot) < soundcraft_model::KitFx::SLOTS);
                for (id, _) in *params {
                    assert!(info.params.iter().any(|p| p.id == *id), "{plugin}: {id}");
                }
            }
        }
        assert_eq!(KITS[0].name, "Porch Light");
        assert!(find("tin roof").is_some() && find("nope").is_none());
    }

    #[test]
    fn a_rendered_hit_is_louder_at_higher_velocity_and_trimmed() {
        let params: std::collections::BTreeMap<String, f32> = JUKE_INFO.params.iter().map(|p| (p.id.to_string(), p.default)).collect();
        let soft = render_pad(&params, 0, 40, 0.0, 48_000).unwrap();
        let hard = render_pad(&params, 0, 127, 0.0, 48_000).unwrap();
        assert!(hard.peak() > soft.peak() * 1.5, "{} vs {}", hard.peak(), soft.peak());
        assert!(hard.frames() < 48_000 * 3 && hard.frames() > 4_800, "{} frames", hard.frames());
        assert!(hard.channels[0].iter().all(|x| x.is_finite()));
        // The hats are short.
        let hat = render_pad(&params, 4, 100, 0.0, 48_000).unwrap();
        assert!(hat.frames() < hard.frames());
    }
}

/// A recorded kit as installed: a folder with `kit.json` and its samples.
///
/// ```json
/// {"name": "SM Drums", "license": "Public domain", "credit": "…",
///  "pads": [{"name": "Kick", "family": "kick", "note": 36, "trim": -2.5,
///            "samples": {"v1.r1": "kick/v1r1.wav", "v4.r2": "kick/v4r2.wav"}}]}
/// ```
///
/// Pads in pad order (at most 16); each sample's key is its velocity layer (1 the softest) and
/// round-robin take, each 1–4; paths are relative to the folder.
#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
pub struct RecordedKit {
    pub name: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub credit: String,
    #[serde(default)]
    pub pads: Vec<RecordedPad>,
    /// Every Juke setting (`p1_tune`, `level`…), in a kit saved from Juke (`mix.kit_save`).
    #[serde(default)]
    pub params: std::collections::BTreeMap<String, f32>,
    /// Its bus effects, as `mix.kit_fx` takes them (`{bus, slot, plugin, params, bypass}`).
    #[serde(default)]
    pub fx: Vec<serde_json::Value>,
}

/// One pad of a [`RecordedKit`].
#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
pub struct RecordedPad {
    pub name: String,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub note: Option<u8>,
    /// The pad's trim in dB (−24 to +24): the kit's balance between its pads, under the fader.
    #[serde(default)]
    pub trim: Option<f32>,
    /// Its choke group (0 none, 1–8); absent: the hats choke each other in group 1, the rest none.
    #[serde(default)]
    pub choke: Option<u8>,
    #[serde(default)]
    pub samples: std::collections::BTreeMap<String, String>,
}

/// Reads and checks `<dir>/kit.json`: at most 16 pads, sample keys `vN.rM` within Juke's layers
/// and round-robins, sample paths plain and inside the folder.
pub fn read_recorded(dir: &std::path::Path) -> Result<RecordedKit, String> {
    let path = dir.join("kit.json");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let kit: RecordedKit = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if kit.name.trim().is_empty() {
        return Err(format!("{}: the kit has no name", path.display()));
    }
    if kit.pads.len() > PADS {
        return Err(format!("{}: {} pads, Juke has {PADS}", path.display(), kit.pads.len()));
    }
    for (i, pad) in kit.pads.iter().enumerate() {
        if let Some(f) = &pad.family
            && DrumFamily::from_id(f).is_none()
        {
            return Err(format!("pad {}: no family `{f}`", i + 1));
        }
        if let Some(t) = pad.trim
            && !(t.is_finite() && (-24.0..=24.0).contains(&t))
        {
            return Err(format!("pad {}: trim {t} dB is outside -24 to +24", i + 1));
        }
        for (key, file) in &pad.samples {
            if layer_robin(key).is_none() {
                return Err(format!("pad {}: `{key}` is not a layer and take (v1.r1 to v4.r4)", i + 1));
            }
            let p = std::path::Path::new(file);
            if file.is_empty() || !p.components().all(|c| matches!(c, std::path::Component::Normal(_))) {
                return Err(format!("pad {}: `{file}` is not a file in the kit's folder", i + 1));
            }
        }
    }
    Ok(kit)
}

/// `vN.rM` as (layer, take), each from 0.
pub fn layer_robin(key: &str) -> Option<(usize, usize)> {
    let (v, r) = key.split_once('.')?;
    let v: usize = v.strip_prefix('v')?.parse().ok()?;
    let r: usize = r.strip_prefix('r')?.parse().ok()?;
    ((1..=LAYERS).contains(&v) && (1..=soundcraft_dsp::plugins::juke::ROBINS).contains(&r)).then(|| (v - 1, r - 1))
}

#[cfg(test)]
mod recorded_tests {
    use super::*;

    #[test]
    fn kit_json_is_checked() {
        let d = std::env::temp_dir().join(format!("fdawm-recorded-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let write = |s: &str| std::fs::write(d.join("kit.json"), s).unwrap();
        write(r#"{"name":"T","pads":[{"name":"Kick","family":"kick","note":36,"samples":{"v1.r1":"k.wav"}}]}"#);
        let k = read_recorded(&d).unwrap();
        assert_eq!(k.pads[0].samples.get("v1.r1").map(String::as_str), Some("k.wav"));
        for bad in [
            r#"{"name":"","pads":[]}"#,
            r#"{"name":"T","pads":[{"name":"K","samples":{"v5.r1":"k.wav"}}]}"#,
            r#"{"name":"T","pads":[{"name":"K","samples":{"v1.r1":"../k.wav"}}]}"#,
            r#"{"name":"T","pads":[{"name":"K","family":"bongo"}]}"#,
            r#"{"name":"T","pads":[{"name":"K","trim":30}]}"#,
        ] {
            write(bad);
            assert!(read_recorded(&d).is_err(), "{bad}");
        }
        assert_eq!(layer_robin("v4.r4"), Some((3, 3)));
        let _ = std::fs::remove_dir_all(&d);
    }
}
