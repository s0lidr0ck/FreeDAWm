//! CleanQ guide templates: the typical problem areas of a source (male vocal, kick in, bass
//! guitar...), each a set of guides to drop on the graph. Not in the C++ plugin: the owner's
//! approved table (`cleanq-guide-templates.md`, next to the repo), its values verbatim.
//!
//! A template guide's centre is the guide's `freq`, its search range `lo`..`hi`, "Dyn" in the usual
//! move sets `dynamic`, its area name goes to `area`, "sounds like" to `note` and the usual move to
//! `hint`. A template only marks areas to look at: its guides propose no gain (0 dB), and the usual
//! move is shown as a hint, never applied.
//!
//! [`apply`] puts a template's guides into a list: added to the guides already there (a guide that
//! is already there is skipped) or replacing them; never past [`MAX_GUIDES`].

use super::clean_q_guides::{Guide, Guides, MAX_GUIDES};

/// One row of a template.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TemplateGuide {
    /// The area's name ("Mud / box").
    pub area: &'static str,
    /// Centre, Hz.
    pub freq: f64,
    /// The search range, Hz.
    pub lo: f64,
    pub hi: f64,
    /// Better handled as a dynamic band (the usual move says "Dyn").
    pub dynamic: bool,
    /// What it sounds like.
    pub sounds_like: &'static str,
    /// The usual move.
    pub usual_move: &'static str,
}

impl TemplateGuide {
    /// The guide it places: no gain, the area, the sound as its note, the move as its hint.
    pub fn guide(&self) -> Guide {
        let mut g = Guide {
            freq_hz: self.freq,
            lo_hz: self.lo,
            hi_hz: self.hi,
            gain_db: 0.0,
            dynamic: self.dynamic,
            note: self.sounds_like.to_string(),
            done: false,
            area: self.area.to_string(),
            hint: self.usual_move.to_string(),
        };
        g.sanitise();
        g
    }
}

/// One source's template.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Template {
    /// Its id (`mix.module_guides {template}`): lower case, underscores.
    pub id: &'static str,
    /// Its name in the picker.
    pub name: &'static str,
    /// The picker's section ("Vocals", "Drums", "Guitars and bass", "Keys").
    pub group: &'static str,
    /// Its guides, centres ascending.
    pub guides: &'static [TemplateGuide],
}

const fn g(area: &'static str, freq: f64, lo: f64, hi: f64, dynamic: bool, sounds_like: &'static str, usual_move: &'static str) -> TemplateGuide {
    TemplateGuide { area, freq, lo, hi, dynamic, sounds_like, usual_move }
}

/// Every template, in the picker's order.
pub const TEMPLATES: &[Template] = &[
    Template {
        id: "male_vocal",
        name: "Male vocal",
        group: "Vocals",
        guides: &[
            g("Rumble / plosives", 80.0, 20.0, 120.0, true, "thumps, stand noise, \"p\" pops", "high-pass; Dyn for plosives"),
            g("Chest boom / proximity", 180.0, 120.0, 250.0, true, "boomy, too close to the mic", "cut 2–4 dB, Dyn"),
            g("Mud", 350.0, 250.0, 500.0, false, "cloudy, boxy room", "cut 2–3 dB"),
            g("Honk / nasal", 1000.0, 800.0, 1500.0, false, "nasal, telephone-y", "narrow cut"),
            g("Harshness", 3000.0, 2500.0, 4000.0, true, "shouty, edgy on loud notes", "Dyn cut"),
            g("Sibilance", 6500.0, 5000.0, 8000.0, true, "\"s\", \"t\", \"sh\" spit", "Dyn (de-ess)"),
            g("Air", 12000.0, 10000.0, 16000.0, false, "open, breathy top", "gentle boost"),
        ],
    },
    Template {
        id: "female_vocal",
        name: "Female vocal",
        group: "Vocals",
        guides: &[
            g("Rumble / plosives", 100.0, 20.0, 150.0, true, "thumps, \"p\" pops", "high-pass; Dyn"),
            g("Proximity warmth", 250.0, 180.0, 350.0, true, "thick, woofy", "cut 2–3 dB, Dyn"),
            g("Mud / box", 450.0, 350.0, 700.0, false, "boxy, cardboard", "cut 2–3 dB"),
            g("Nasal", 1200.0, 900.0, 1800.0, false, "honky, nasal", "narrow cut"),
            g("Harshness / shrill", 3500.0, 2500.0, 5000.0, true, "piercing on belts", "Dyn cut"),
            g("Sibilance", 7500.0, 6000.0, 10000.0, true, "sharp \"s\"", "Dyn (de-ess)"),
            g("Air", 13000.0, 10000.0, 18000.0, false, "sparkle, breath", "gentle boost"),
        ],
    },
    Template {
        id: "kick_in",
        name: "Kick in",
        group: "Drums",
        guides: &[
            g("Sub thump", 55.0, 40.0, 70.0, false, "weight you feel", "boost if thin"),
            g("Boom / ring", 100.0, 80.0, 150.0, true, "woolly, long tail", "Dyn cut"),
            g("Box / cardboard", 350.0, 250.0, 500.0, false, "papery, boxy", "cut 3–6 dB"),
            g("Beater click", 3500.0, 2000.0, 5000.0, false, "attack, definition", "boost"),
            g("Bleed (snare/cymbals)", 8000.0, 6000.0, 12000.0, false, "splash on the kick", "low-pass / cut"),
        ],
    },
    Template {
        id: "kick_out",
        name: "Kick out",
        group: "Drums",
        guides: &[
            g("Sub / round low", 50.0, 30.0, 70.0, false, "big, round low end", "boost"),
            g("Boom", 90.0, 70.0, 130.0, true, "flabby", "Dyn cut"),
            g("Room / box", 400.0, 250.0, 600.0, false, "hollow, roomy", "cut"),
            g("Bleed", 5000.0, 3000.0, 10000.0, false, "cymbals in the kick", "low-pass"),
        ],
    },
    Template {
        id: "snare_top",
        name: "Snare top",
        group: "Drums",
        guides: &[
            g("Kick/tom bleed", 80.0, 40.0, 120.0, false, "low thud not from the snare", "high-pass"),
            g("Body / fatness", 200.0, 150.0, 250.0, false, "weight of the hit", "boost if thin"),
            g("Ring / box", 500.0, 400.0, 900.0, true, "ping, boxy ring", "narrow Dyn cut"),
            g("Crack", 2500.0, 1500.0, 4000.0, false, "stick attack", "boost"),
            g("Hi-hat bleed", 6000.0, 5000.0, 10000.0, true, "hat sitting in the snare", "cut / Dyn"),
            g("Snap / air", 10000.0, 8000.0, 12000.0, false, "crispness", "gentle boost"),
        ],
    },
    Template {
        id: "snare_bottom",
        name: "Snare bottom",
        group: "Drums",
        guides: &[
            g("Kick bleed", 100.0, 40.0, 200.0, false, "kick in the snare bottom", "high-pass hard"),
            g("Mid ring", 600.0, 400.0, 1000.0, false, "honky ring", "cut"),
            g("Wire buzz / rattle", 4000.0, 3000.0, 7000.0, true, "sympathetic rattle", "Dyn cut"),
            g("Sizzle", 8000.0, 6000.0, 10000.0, false, "wire sizzle", "boost to taste"),
        ],
    },
    Template {
        id: "rack_tom",
        name: "Rack tom",
        group: "Drums",
        guides: &[
            g("Fundamental", 120.0, 90.0, 200.0, false, "the tom's note", "boost slightly"),
            g("Box / mud", 450.0, 300.0, 700.0, false, "cardboard, boxy", "cut"),
            g("Ring / overtones", 900.0, 700.0, 1500.0, true, "long ringing", "narrow Dyn cut"),
            g("Attack", 4000.0, 3000.0, 6000.0, false, "stick definition", "boost"),
            g("Cymbal bleed", 8000.0, 7000.0, 12000.0, false, "crash wash in the tom", "cut"),
        ],
    },
    Template {
        id: "floor_tom",
        name: "Floor tom",
        group: "Drums",
        guides: &[
            g("Fundamental", 80.0, 60.0, 120.0, false, "the tom's note, weight", "boost slightly"),
            g("Mud", 350.0, 250.0, 500.0, false, "boxy, thick", "cut"),
            g("Ring", 700.0, 500.0, 1000.0, true, "sustained ring", "narrow Dyn cut"),
            g("Attack", 3500.0, 2500.0, 5000.0, false, "stick definition", "boost"),
            g("Cymbal bleed", 8000.0, 7000.0, 12000.0, false, "cymbal wash", "cut"),
        ],
    },
    Template {
        id: "overheads",
        name: "Overheads",
        group: "Drums",
        guides: &[
            g("Low bleed", 100.0, 40.0, 200.0, false, "kick/tom rumble", "high-pass (keep if OH carry the kit)"),
            g("Mud / room", 400.0, 300.0, 600.0, false, "boxy kit", "cut"),
            g("Clang / gong", 1500.0, 1000.0, 2500.0, true, "gongy crashes", "Dyn cut"),
            g("Harsh cymbals", 3500.0, 2500.0, 5000.0, true, "brittle, painful crashes", "Dyn cut"),
            g("Shimmer / air", 12000.0, 10000.0, 16000.0, false, "sheen", "gentle boost"),
        ],
    },
    Template {
        id: "hi_hat",
        name: "Hi-hat",
        group: "Drums",
        guides: &[
            g("Bleed", 200.0, 40.0, 400.0, false, "snare/kick in the hat", "high-pass"),
            g("Clang", 1000.0, 600.0, 2000.0, false, "clanky, metallic", "cut"),
            g("Harsh", 5000.0, 3000.0, 7000.0, true, "spitty chick", "Dyn cut"),
            g("Sizzle", 12000.0, 10000.0, 16000.0, false, "shimmer", "to taste"),
        ],
    },
    Template {
        id: "room_mic",
        name: "Room mic",
        group: "Drums",
        guides: &[
            g("Rumble", 50.0, 20.0, 80.0, false, "HVAC, traffic", "high-pass"),
            g("Low build-up", 150.0, 100.0, 250.0, true, "boomy room modes", "Dyn cut"),
            g("Mud / box", 400.0, 250.0, 600.0, false, "small-room boxiness", "cut"),
            g("Harsh reflections", 2500.0, 2000.0, 4000.0, true, "splashy, painful when compressed", "Dyn cut"),
            g("Cymbal wash", 8000.0, 6000.0, 12000.0, false, "cymbals overwhelm the room", "cut / low-pass"),
        ],
    },
    Template {
        id: "acoustic_guitar",
        name: "Acoustic guitar",
        group: "Guitars and bass",
        guides: &[
            g("Body boom", 120.0, 80.0, 200.0, true, "boomy soundhole", "cut / high-pass, Dyn"),
            g("Mud", 300.0, 200.0, 450.0, false, "woolly strumming", "cut"),
            g("Box", 800.0, 600.0, 1200.0, false, "boxy, honky", "narrow cut"),
            g("Pick / harsh", 3000.0, 2000.0, 4000.0, true, "clacky pick", "Dyn cut"),
            g("Fret squeak / string noise", 5000.0, 4000.0, 7000.0, true, "squeaks on slides", "Dyn cut"),
            g("Sparkle", 12000.0, 9000.0, 15000.0, false, "shimmer", "gentle boost"),
        ],
    },
    Template {
        id: "electric_guitar_rhythm",
        name: "Electric guitar (rhythm)",
        group: "Guitars and bass",
        guides: &[
            g("Low rumble", 80.0, 30.0, 100.0, false, "competes with bass/kick", "high-pass"),
            g("Cab boom / proximity", 150.0, 100.0, 250.0, false, "thick, woofy", "cut"),
            g("Mud", 350.0, 250.0, 500.0, false, "muddy chugs", "cut"),
            g("Honk", 1000.0, 700.0, 1500.0, false, "honky, nasal", "cut"),
            g("Fizz / harsh", 4000.0, 3000.0, 6000.0, true, "fizzy distortion", "cut / Dyn"),
            g("Hiss / noise", 10000.0, 8000.0, 15000.0, false, "amp hiss", "low-pass"),
        ],
    },
    Template {
        id: "lead_guitar",
        name: "Lead guitar",
        group: "Guitars and bass",
        guides: &[
            g("Low clutter", 100.0, 40.0, 150.0, false, "unneeded lows under a solo", "high-pass"),
            g("Mud", 300.0, 200.0, 450.0, false, "woolly", "cut"),
            g("Presence / cut-through", 1500.0, 1000.0, 2500.0, false, "where the solo speaks", "boost"),
            g("Piercing", 3500.0, 2500.0, 5000.0, true, "ice-pick on high notes", "Dyn cut"),
            g("Fizz", 6000.0, 5000.0, 9000.0, false, "buzzy top", "cut"),
        ],
    },
    Template {
        id: "bass_guitar",
        name: "Bass guitar",
        group: "Guitars and bass",
        guides: &[
            g("Sub rumble", 35.0, 20.0, 45.0, false, "unheard energy, eats headroom", "high-pass"),
            g("Weight / fundamental", 80.0, 60.0, 100.0, false, "the bottom", "keep / boost"),
            g("Boom / mud", 200.0, 150.0, 300.0, true, "boomy, masks kick", "Dyn cut"),
            g("Growl / definition", 800.0, 600.0, 1200.0, false, "note definition", "boost"),
            g("String / finger noise", 2500.0, 1500.0, 4000.0, true, "clank, finger squeak", "Dyn cut"),
            g("Hiss / amp noise", 8000.0, 6000.0, 12000.0, false, "noise above the instrument", "low-pass"),
        ],
    },
    Template {
        id: "piano",
        name: "Piano",
        group: "Keys",
        guides: &[
            g("Thump / pedal noise", 60.0, 20.0, 90.0, false, "pedal and lid thumps", "high-pass"),
            g("Boom", 150.0, 100.0, 250.0, true, "thick left hand", "Dyn cut"),
            g("Mud / box", 400.0, 250.0, 600.0, false, "boxy, cloudy chords", "cut"),
            g("Honk", 1000.0, 800.0, 1500.0, false, "honky upright", "narrow cut"),
            g("Harsh hammers", 3000.0, 2000.0, 5000.0, true, "clangy top octaves", "Dyn cut"),
            g("Air", 12000.0, 10000.0, 16000.0, false, "shimmer", "gentle boost"),
        ],
    },
];

/// The template with this id or name (either case), if any.
pub fn template(key: &str) -> Option<&'static Template> {
    let key = key.trim();
    TEMPLATES.iter().find(|t| t.id.eq_ignore_ascii_case(key) || t.name.eq_ignore_ascii_case(key))
}

/// What [`apply`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Applied {
    /// Guides placed.
    pub added: usize,
    /// Guides already there (skipped).
    pub skipped: usize,
    /// Guides left out because the list was full.
    pub full: usize,
}

/// `g` is already in the list: a guide within a semitone of its centre, or one with its area name
/// whose range overlaps its range.
fn already_there(list: &[Guide], g: &Guide) -> bool {
    let semitone = 2f64.ln() / 12.0;
    list.iter().any(|x| {
        let near = (x.freq_hz.max(1e-9) / g.freq_hz.max(1e-9)).ln().abs() < semitone;
        let same_area = !g.area.is_empty() && x.area.eq_ignore_ascii_case(&g.area) && x.lo_hz < g.hi_hz && g.lo_hz < x.hi_hz;
        near || same_area
    })
}

/// Puts `t`'s guides into `guides`: replacing them (`replace`) or added to them, skipping the ones
/// already there; at most [`MAX_GUIDES`] in all. The list stays in the order the guides were placed.
pub fn apply(guides: &mut Guides, t: &Template, replace: bool) -> Applied {
    if replace {
        guides.list.clear();
    }
    let mut out = Applied::default();
    for tg in t.guides {
        let gd = tg.guide();
        if already_there(&guides.list, &gd) {
            out.skipped += 1;
        } else if guides.list.len() >= MAX_GUIDES {
            out.full += 1;
        } else if guides.add(gd).is_some() {
            out.added += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::fs::cleanq::clean_q_guides::{FREQ_MAX, FREQ_MIN};

    #[test]
    fn every_template_is_valid() {
        assert_eq!(TEMPLATES.len(), 16);
        let mut ids = std::collections::HashSet::new();
        for t in TEMPLATES {
            assert!(ids.insert(t.id), "{} twice", t.id);
            assert!(!t.guides.is_empty() && t.guides.len() <= MAX_GUIDES, "{}: {} guides", t.id, t.guides.len());
            assert!(t.id.chars().all(|c| c.is_ascii_lowercase() || c == '_'), "{}", t.id);
            for w in t.guides.windows(2) {
                assert!(w[0].freq < w[1].freq, "{}: {} then {}", t.id, w[0].area, w[1].area);
            }
            for tg in t.guides {
                assert!(FREQ_MIN <= tg.lo && tg.lo < tg.freq && tg.freq < tg.hi && tg.hi <= FREQ_MAX, "{}: {}", t.id, tg.area);
                assert!(!tg.area.is_empty() && !tg.sounds_like.is_empty() && !tg.usual_move.is_empty());
                assert_eq!(tg.dynamic, tg.usual_move.contains("Dyn"), "{}: {}", t.id, tg.area);
                // The guide carries it all, unchanged by sanitising, and proposes no gain.
                let gd = tg.guide();
                assert_eq!((gd.freq_hz, gd.lo_hz, gd.hi_hz, gd.gain_db), (tg.freq, tg.lo, tg.hi, 0.0));
                assert_eq!((gd.area.as_str(), gd.note.as_str(), gd.hint.as_str()), (tg.area, tg.sounds_like, tg.usual_move));
            }
            assert_eq!(template(t.id), Some(t));
            assert_eq!(template(&t.name.to_uppercase()), Some(t));
        }
        assert!(template("kazoo").is_none());
    }

    #[test]
    fn the_female_vocal_rows_verbatim() {
        let t = template("female_vocal").map(|t| t.guides).unwrap_or_default();
        let rows: Vec<(&str, f64, f64, f64, bool)> = t.iter().map(|g| (g.area, g.freq, g.lo, g.hi, g.dynamic)).collect();
        assert_eq!(
            rows,
            vec![
                ("Rumble / plosives", 100.0, 20.0, 150.0, true),
                ("Proximity warmth", 250.0, 180.0, 350.0, true),
                ("Mud / box", 450.0, 350.0, 700.0, false),
                ("Nasal", 1200.0, 900.0, 1800.0, false),
                ("Harshness / shrill", 3500.0, 2500.0, 5000.0, true),
                ("Sibilance", 7500.0, 6000.0, 10000.0, true),
                ("Air", 13000.0, 10000.0, 18000.0, false),
            ]
        );
        assert_eq!((t[1].sounds_like, t[1].usual_move), ("thick, woofy", "cut 2\u{2013}3 dB, Dyn"));
    }

    #[test]
    fn apply_adds_skips_replaces_and_stops_at_the_limit() {
        let vox = template("male_vocal").copied().unwrap_or(TEMPLATES[0]);
        let mut gs = Guides::default();
        gs.add(Guide { note: "mine".into(), ..Guide::around(352.0, -3.0) });
        let a = apply(&mut gs, &vox, false);
        assert_eq!(a, Applied { added: 6, skipped: 1, full: 0 }, "Mud (350 Hz) is already there");
        assert_eq!(gs.list[0].note, "mine", "the guides already there are kept");
        assert_eq!(apply(&mut gs, &vox, false), Applied { added: 0, skipped: 7, full: 0 }, "twice adds nothing");
        let a = apply(&mut gs, &vox, true);
        assert_eq!(a.added, 7);
        assert_eq!(gs.list.len(), 7);
        assert!(gs.list.iter().all(|g| g.gain_db == 0.0 && !g.done));
        // Nearly full: 20 of your own (none near the piano's centres), then a 6-guide template.
        let mut gs = Guides::default();
        for i in 0..20 {
            gs.add(Guide::around(20_000.0 - f64::from(i) * 100.0, -3.0));
        }
        let piano = template("piano").copied().unwrap_or(TEMPLATES[0]);
        let a = apply(&mut gs, &piano, false);
        assert_eq!(gs.list.len(), MAX_GUIDES);
        assert_eq!(a, Applied { added: 4, skipped: 0, full: 2 });
    }
}
