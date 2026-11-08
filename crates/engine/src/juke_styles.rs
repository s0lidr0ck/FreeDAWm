//! Juke's styles: for each, a kit ([`crate::juke_kits::KITS`], named after the style) and starter
//! grooves, original by construction.
//!
//! A groove is one line of steps per instrument, on the default kit's General MIDI notes (the
//! pads' notes are not changed by loading a kit). Each character is a step:
//!
//! | char | step |
//! |---|---|
//! | `X` | accented hit (velocity 120, accent on) |
//! | `x` | hit (100) |
//! | `o` | soft hit (70) |
//! | `g` | ghost note (40) |
//! | `r` | roll (two hits in the step, 90) |
//! | `f` | flam (100, a grace hit before) |
//! | `.` | rest |
//!
//! A line is one bar: as many characters as the groove's steps per bar (16, or 12 for a triplet
//! feel); `|` and spaces are only for reading.

use soundcraft_midi::pattern::{Lane, Pattern, Step};

/// General MIDI drum notes on the default kit's pads.
pub mod note {
    pub const KICK: u8 = 36;
    pub const RIM: u8 = 37;
    pub const SNARE: u8 = 38;
    pub const CLAP: u8 = 39;
    pub const HAT: u8 = 42;
    pub const PEDAL: u8 = 44;
    pub const OPEN: u8 = 46;
    pub const LOW_TOM: u8 = 41;
    pub const MID_TOM: u8 = 47;
    pub const HIGH_TOM: u8 = 50;
    pub const CRASH: u8 = 49;
    pub const RIDE: u8 = 51;
    pub const TAMB: u8 = 54;
    pub const COWBELL: u8 = 56;
    pub const SHAKER: u8 = 70;
    pub const CLAVE: u8 = 75;
}
use note::*;

/// A starter groove.
pub struct Groove {
    pub name: &'static str,
    /// 16 (sixteenths) or 12 (a triplet feel).
    pub steps_per_bar: u8,
    /// Swing on the off-beats, 0–100 %.
    pub shuffle: u8,
    /// (note, one bar of steps).
    pub lanes: &'static [(u8, &'static str)],
}

/// A style: its kit (by name) and its grooves, the first the default.
pub struct Style {
    pub name: &'static str,
    /// What the menus group it under.
    pub family: &'static str,
    pub kit: &'static str,
    pub grooves: &'static [Groove],
}

const fn g(name: &'static str, lanes: &'static [(u8, &'static str)]) -> Groove {
    Groove { name, steps_per_bar: 16, shuffle: 0, lanes }
}

const fn swung(name: &'static str, shuffle: u8, lanes: &'static [(u8, &'static str)]) -> Groove {
    Groove { name, steps_per_bar: 16, shuffle, lanes }
}

const fn triplet(name: &'static str, lanes: &'static [(u8, &'static str)]) -> Groove {
    Groove { name, steps_per_bar: 12, shuffle: 0, lanes }
}

pub static STYLES: &[Style] = &[
    // Hip-hop and electronic.
    Style {
        name: "Boom Bap",
        family: "Hip-Hop",
        kit: "Boom Bap",
        grooves: &[
            swung("Classic", 55, &[(KICK, "X......x .X......"), (SNARE, "....X... ....X..."), (HAT, "x.x.x.x. x.x.x.x.")]),
            swung("Busy Hats", 55, &[(KICK, "X.....x. ..X..x.."), (SNARE, "....X..g ....X..."), (HAT, "xoxoxoxo xoxoxoxo")]),
            swung("Half-Time", 55, &[(KICK, "X....... ..x....."), (SNARE, "........ X......."), (HAT, "x.x.x.x. x.x.x.x.")]),
        ],
    },
    Style {
        name: "Trap",
        family: "Hip-Hop",
        kit: "Trap",
        grooves: &[
            g("Straight", &[(KICK, "X......X ..X....."), (SNARE, "........ X......."), (HAT, "x.x.x.x. x.x.rrx.")]),
            Groove {
                name: "Triplet Rolls",
                steps_per_bar: 24,
                shuffle: 0,
                lanes: &[(KICK, "X........... X.....X....."), (SNARE, "............ X..........."), (HAT, "x.x.x.x.x.x. xxxxxxx.x.x.")],
            },
            g("Sparse", &[(KICK, "X....... ..X....."), (SNARE, "........ X......."), (HAT, "x...x... x...x..."), (OPEN, "........ ......x.")]),
        ],
    },
    Style {
        name: "Lo-fi",
        family: "Hip-Hop",
        kit: "Lo-fi",
        grooves: &[
            swung("Lazy Swing", 60, &[(KICK, "X......x ..X....."), (SNARE, "....X... ....X..."), (HAT, "o.o.o.o. o.o.o.o.")]),
            swung("Rimshot", 60, &[(KICK, "X....... X.x....."), (RIM, "....X... ....X..."), (SHAKER, "x.x.x.x. x.x.x.x.")]),
        ],
    },
    Style {
        name: "House",
        family: "Electronic",
        kit: "House",
        grooves: &[
            g("Classic", &[(KICK, "X...X... X...X..."), (CLAP, "....X... ....X..."), (OPEN, "..x...x. ..x...x."), (HAT, "g.g.g.g. g.g.g.g.")]),
            swung(
                "Shuffled",
                60,
                &[(KICK, "X...X... X...X..."), (CLAP, "....X... ....X..."), (OPEN, "..x...x. ..x...x."), (SHAKER, "xoxoxoxo xoxoxoxo")],
            ),
            g(
                "Tribal",
                &[(KICK, "X...X... X...X..."), (LOW_TOM, "..x....x ..x....."), (MID_TOM, "......x. ......x."), (COWBELL, "x..x..x. .x..x...")],
            ),
        ],
    },
    Style {
        name: "Techno",
        family: "Electronic",
        kit: "Techno",
        grooves: &[
            g("Driving", &[(KICK, "X...X... X...X..."), (RIM, "..x..x.. ..x..x.."), (OPEN, "..x...x. ..x...x."), (RIDE, "o.o.o.o. o.o.o.o.")]),
            g("Broken", &[(KICK, "X..X..X. ..X..x.."), (CLAP, "....X... ....X..."), (HAT, "xoxoxoxo xoxoxoxo")]),
        ],
    },
    Style {
        name: "Drum & Bass",
        family: "Electronic",
        kit: "Drum & Bass",
        grooves: &[
            g("Two-Step", &[(KICK, "X....... ..X....."), (SNARE, "....X... ....X..."), (HAT, "x.x.x.x. x.x.x.x.")]),
            g("Roller", &[(KICK, "X.....x. ..X....."), (SNARE, "....X..g .g..X..."), (RIDE, "x.x.x.x. x.x.x.x."), (HAT, "..x...x. ..x...x.")]),
        ],
    },
    Style {
        name: "Reggaeton",
        family: "Latin",
        kit: "Reggaeton",
        grooves: &[
            g("Dembow", &[(KICK, "X...X... X...X..."), (SNARE, "...x..x. ...x..x."), (HAT, "x.x.x.x. x.x.x.x.")]),
            g("Perreo", &[(KICK, "X...X... X...X..."), (SNARE, "...x..x. x..x..x."), (CLAP, "...x.... ..x....."), (HAT, "x.x.x.x. x.x.x.x.")]),
        ],
    },
    Style {
        name: "Funk",
        family: "Soul & Funk",
        kit: "Funk",
        grooves: &[
            g("Syncopated", &[(KICK, "X..X..x. ..X..x.."), (SNARE, "....X..g .g..X..g"), (HAT, "oooooooo oooooooo"), (OPEN, "........ ......x.")]),
            g("Ghosted", &[(KICK, "X.....X. ..X....."), (SNARE, "g.g.X.g. .gg.X.g."), (HAT, "x.x.x.x. x.x.x.x.")]),
        ],
    },
    // Rock.
    Style {
        name: "Alternative",
        family: "Rock",
        kit: "Alternative",
        grooves: &[
            g("Straight 8ths", &[(KICK, "X.....x. X......."), (SNARE, "....X... ....X..."), (HAT, "x.x.x.x. x.x.x.x.")]),
            g("Driving 16ths", &[(KICK, "X..x..x. X..x...."), (SNARE, "....X... ....X..."), (HAT, "xoxoxoxo xoxoxoxo")]),
            g("Tom Groove", &[(KICK, "X....... X......."), (LOW_TOM, "X..x..x. X..x..x."), (SNARE, "....X... ....X...")]),
        ],
    },
    Style {
        name: "Soft Rock",
        family: "Rock",
        kit: "Soft Rock",
        grooves: &[
            g("Ballad", &[(KICK, "X....... x.x....."), (SNARE, "....X... ....X..."), (HAT, "o.o.o.o. o.o.o.o.")]),
            g("Cross-Stick", &[(KICK, "X.....x. ..x....."), (RIM, "....X... ....X..."), (HAT, "o.o.o.o. o.o.o.o.")]),
        ],
    },
    Style {
        name: "Hard Rock",
        family: "Rock",
        kit: "Hard Rock",
        grooves: &[
            g("Rock Beat", &[(KICK, "X.x..... X.x....."), (SNARE, "....X... ....X..."), (HAT, "x.x.x.x. x.x.x.x."), (CRASH, "X....... ........")]),
            g("Ride Groove", &[(KICK, "X.....x. X.x....."), (SNARE, "....X... ....X..."), (RIDE, "x.x.x.x. x.x.x.x.")]),
            g("Fill", &[(KICK, "X....... ........"), (SNARE, "....X... xxxx...."), (HIGH_TOM, "........ ....xx.."), (LOW_TOM, "........ ......xx")]),
        ],
    },
    Style {
        name: "Metal",
        family: "Rock",
        kit: "Metal",
        grooves: &[
            g("Double Kick", &[(KICK, "xxxxxxxx xxxxxxxx"), (SNARE, "....X... ....X..."), (HAT, "x.x.x.x. x.x.x.x."), (CRASH, "X....... ........")]),
            g("Gallop", &[(KICK, "X.xxX.xx X.xxX.xx"), (SNARE, "....X... ....X..."), (RIDE, "x.x.x.x. x.x.x.x.")]),
            g("Blast Beat", &[(KICK, "x.x.x.x. x.x.x.x."), (SNARE, ".x.x.x.x .x.x.x.x"), (CRASH, "x.x.x.x. x.x.x.x.")]),
        ],
    },
    Style {
        name: "'50s Rock",
        family: "Rock",
        kit: "'50s Rock",
        grooves: &[
            swung("Backbeat", 60, &[(KICK, "X....... X......."), (SNARE, "....X... ....X..."), (RIDE, "x...x.x. x...x.x.")]),
            triplet("Shuffle", &[(KICK, "X.....X....."), (SNARE, "...X.....X.."), (RIDE, "X.xX.xX.xX.x")]),
        ],
    },
    Style {
        name: "'60s Rock",
        family: "Rock",
        kit: "'60s Rock",
        grooves: &[
            g("Beat Group", &[(KICK, "X.x..... X.x....."), (SNARE, "....X... ....X..."), (HAT, "x.x.x.x. x.x.x.x."), (TAMB, "....x... ....x...")]),
            g("Surf", &[(KICK, "X...X... X...X..."), (LOW_TOM, "Xoooxooo Xoooxooo"), (SNARE, "....X... ....X...")]),
        ],
    },
    Style {
        name: "Rockabilly",
        family: "Rock",
        kit: "Rockabilly",
        grooves: &[
            swung("Slap Back", 55, &[(KICK, "X....... X......."), (SNARE, "..x.X.x. ..x.X.x."), (HAT, "x.x.x.x. x.x.x.x.")]),
            triplet("Boogie", &[(KICK, "X..X..X..X.."), (SNARE, "...X.....X.."), (RIDE, "X.xX.xX.xX.x")]),
        ],
    },
    // Pop, folk and country.
    Style {
        name: "Pop",
        family: "Pop & Folk",
        kit: "Pop",
        grooves: &[
            g("Four on the Floor", &[(KICK, "X...X... X...X..."), (CLAP, "....X... ....X..."), (HAT, "x.x.x.x. x.x.x.x.")]),
            g("Pop Ballad", &[(KICK, "X....... x.x....."), (SNARE, "....X... ....X..."), (HAT, "o.o.o.o. o.o.o.o."), (SHAKER, "xoxoxoxo xoxoxoxo")]),
            g("Dance Pop", &[(KICK, "X..x..x. X......."), (CLAP, "....X... ....X..."), (HAT, "xoxoxoxo xoxoxoxo"), (OPEN, "..x...x. ..x...x.")]),
        ],
    },
    Style {
        name: "Folk",
        family: "Pop & Folk",
        kit: "Folk",
        grooves: &[
            g("Boom-Chick", &[(KICK, "X....... X......."), (SNARE, "....X... ....X..."), (TAMB, "x.x.x.x. x.x.x.x.")]),
            g("Stomp & Clap", &[(KICK, "X....... X......."), (CLAP, "....X... ....X..."), (TAMB, "..x...x. ..x...x.")]),
            triplet("Six-Eight", &[(KICK, "X.....X....."), (SNARE, "...x.....x.."), (SHAKER, "x.xx.xx.xx.x")]),
        ],
    },
    Style {
        name: "Country",
        family: "Pop & Folk",
        kit: "Country",
        grooves: &[
            g("Train Beat", &[(KICK, "X....... X......."), (SNARE, "ggggXggg ggggXggg")]),
            swung("Shuffle", 66, &[(KICK, "X....... X......."), (SNARE, "....X... ....X..."), (HAT, "x.x.x.x. x.x.x.x.")]),
        ],
    },
    // Church.
    Style {
        name: "Praise",
        family: "Church",
        kit: "Praise",
        grooves: &[
            g("Upbeat", &[(KICK, "X.x...x. X.x....."), (SNARE, "....X... ....X..."), (HAT, "x.x.x.x. x.x.x.x."), (TAMB, "....x... ....x...")]),
            g(
                "Four on the Floor",
                &[(KICK, "X...X... X...X..."), (SNARE, "....X... ....X..."), (OPEN, "..x...x. ..x...x."), (HAT, "x.x.x.x. x.x.x.x.")],
            ),
        ],
    },
    Style {
        name: "Worship",
        family: "Church",
        kit: "Worship",
        grooves: &[
            g("Build", &[(KICK, "X...X... X...X..."), (SNARE, "........ X......."), (LOW_TOM, "X..x..x. X..x..x."), (RIDE, "x.x.x.x. x.x.x.x.")]),
            g("Ballad", &[(KICK, "X....... ..x....."), (SNARE, "........ X......."), (HAT, "o.o.o.o. o.o.o.o.")]),
            triplet("Six-Eight Swell", &[(KICK, "X.....X....."), (SNARE, "......X....."), (RIDE, "x.xx.xx.xx.x")]),
        ],
    },
    Style {
        name: "Gospel / Shout",
        family: "Church",
        kit: "Gospel / Shout",
        grooves: &[
            g("Shout", &[(KICK, "X..X.X.. X..X.X.."), (SNARE, "g.g.X.g. g.g.X.g."), (PEDAL, "....x... ....x..."), (HAT, "x.x.x.x. x.x.x.x.")]),
            g("Gospel Chops", &[(KICK, "X.....x. X..x...."), (SNARE, "....X..g .g..X..."), (CLAP, "....X... ....X..."), (HAT, "x.x.x.x. x.x.x.x.")]),
            triplet("Shout Shuffle", &[(KICK, "X..X..X..X.."), (SNARE, "..x..X..x..X"), (HAT, "x..x..x..x..")]),
        ],
    },
];

/// The style named `name` (any case).
pub fn find(name: &str) -> Option<&'static Style> {
    STYLES.iter().find(|s| s.name.eq_ignore_ascii_case(name.trim()))
}

impl Style {
    /// The groove named `name` (any case), or numbered `name` from 1.
    pub fn groove(&self, name: &str) -> Option<&'static Groove> {
        let name = name.trim();
        // `self` is one of STYLES ('static), so its grooves are too.
        let grooves: &'static [Groove] = self.grooves;
        match name.parse::<usize>() {
            Ok(n) if n >= 1 => grooves.get(n - 1),
            _ => grooves.iter().find(|g| g.name.eq_ignore_ascii_case(name)),
        }
    }
}

/// One step from its character (see the module notes); `None` for a rest or an unknown character.
fn step(c: char) -> Option<Step> {
    let mut s = match c {
        'X' => Step { accent: true, ..Step::hit(120) },
        'x' => Step::hit(100),
        'o' => Step::hit(70),
        'g' => Step::hit(40),
        'r' => Step { roll: 2, ..Step::hit(90) },
        'f' => Step { flam: true, ..Step::hit(100) },
        _ => return None,
    };
    s.probability = 100;
    Some(s)
}

/// A lane's steps, the reading characters (`|`, spaces) left out.
pub fn steps_of(line: &str) -> Vec<Step> {
    line.chars().filter(|c| !matches!(c, '|' | ' ')).map(|c| step(c).unwrap_or_default()).collect()
}

/// The pattern a groove makes, with `seed` for its chances.
pub fn pattern(g: &Groove, seed: u64) -> Pattern {
    let lanes = g
        .lanes
        .iter()
        .map(|(note, line)| {
            let mut lane = Lane::new(*note, usize::from(g.steps_per_bar));
            lane.steps = steps_of(line);
            lane
        })
        .collect();
    let mut p = Pattern { steps_per_bar: g.steps_per_bar, shuffle: g.shuffle, seed, lanes, ..Pattern::default() };
    p.sanitize();
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::juke_kits::KITS;

    const PAD_NOTES: [u8; 16] = [KICK, RIM, SNARE, CLAP, HAT, PEDAL, OPEN, LOW_TOM, MID_TOM, HIGH_TOM, CRASH, RIDE, TAMB, COWBELL, SHAKER, CLAVE];

    #[test]
    fn every_style_has_its_kit_and_well_formed_grooves() {
        assert!(STYLES.len() >= 21);
        for s in STYLES {
            assert!(KITS.iter().any(|k| k.name == s.kit), "{}: no kit {}", s.name, s.kit);
            assert!(!s.grooves.is_empty(), "{}", s.name);
            for g in s.grooves {
                assert!(soundcraft_midi::pattern::STEPS_PER_BAR.contains(&g.steps_per_bar), "{} / {}", s.name, g.name);
                for (n, line) in g.lanes {
                    assert!(PAD_NOTES.contains(n), "{} / {}: note {n} is on no pad", s.name, g.name);
                    let chars: Vec<char> = line.chars().filter(|c| !matches!(c, '|' | ' ')).collect();
                    assert_eq!(chars.len(), usize::from(g.steps_per_bar), "{} / {}: lane {n} is not one bar", s.name, g.name);
                    assert!(chars.iter().all(|c| "Xxogrf.".contains(*c)), "{} / {}: lane {n}", s.name, g.name);
                    assert!(chars.iter().any(|c| *c != '.'), "{} / {}: lane {n} is empty", s.name, g.name);
                }
                let p = pattern(g, 7);
                assert_eq!(p.lanes.len(), g.lanes.len());
                assert!(p.lanes.iter().all(|l| l.steps.len() == usize::from(g.steps_per_bar)));
            }
        }
    }

    #[test]
    fn the_requested_styles_are_all_there() {
        for name in [
            "Alternative",
            "Soft Rock",
            "Hard Rock",
            "Metal",
            "'50s Rock",
            "'60s Rock",
            "Rockabilly",
            "Pop",
            "Folk",
            "Praise",
            "Worship",
            "Gospel / Shout",
        ] {
            assert!(find(name).is_some(), "{name}");
        }
        let s = find("trap").unwrap();
        assert_eq!(s.groove("2").map(|g| g.name), Some("Triplet Rolls"));
        assert_eq!(s.groove("sparse").map(|g| g.name), Some("Sparse"));
        assert!(s.groove("0").is_none() && s.groove("nope").is_none());
    }

    #[test]
    fn step_characters_make_their_hits() {
        let v = steps_of("Xxog|rf.");
        assert_eq!(
            v.iter().map(|s| (s.on, s.velocity)).collect::<Vec<_>>(),
            [(true, 120), (true, 100), (true, 70), (true, 40), (true, 90), (true, 100), (false, 100)]
        );
        assert!(v[0].accent && v[4].roll == 2 && v[5].flam);
    }
}
