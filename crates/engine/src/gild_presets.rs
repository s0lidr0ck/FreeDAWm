//! Gild's built-in presets: one effect's settings, or a whole gilding's set of effects. Names and
//! values are FreeDAWm's own.

use soundcraft_model::{GildDouble, GildEcho, GildFx, GildGrit, GildNote, GildShift, GildSpace, GildSwell, GildVerb, GritFlavour};

/// A preset: its name, what it sets (`delay`, `reverb`, … or `gilding` for a whole set) and the
/// effects it turns on.
#[derive(Debug, Clone)]
pub struct GildPreset {
    pub name: &'static str,
    pub kind: &'static str,
    pub fx: GildFx,
}

fn fx() -> GildFx {
    GildFx::default()
}

fn echo(note: GildNote, repeats: f32, low: f32, high: f32, bounce: bool, level_db: f32) -> GildEcho {
    GildEcho { sync: true, note, ms: 360.0, repeats, low_cut: low, high_cut: high, bounce, level_db }
}

fn verb(space: GildSpace, size: f32, decay_s: f32, pre_ms: f32, width: f32, level_db: f32) -> GildVerb {
    GildVerb { space, size, decay_s, pre_ms, width, level_db }
}

/// Every built-in preset, effects first (in the order the effects are heard), then whole
/// gildings.
pub fn all() -> Vec<GildPreset> {
    let p = |name, kind, fx| GildPreset { name, kind, fx };
    vec![
        p("Octave-below layer", "shift", GildFx { shift: Some(GildShift { semitones: -12.0, formant: 0.0, layer: true, level_db: -7.0 }), ..fx() }),
        p("Fifth above layer", "shift", GildFx { shift: Some(GildShift { semitones: 7.0, formant: 0.0, layer: true, level_db: -10.0 }), ..fx() }),
        p("Giant voice", "shift", GildFx { shift: Some(GildShift { semitones: -4.0, formant: -3.0, layer: false, level_db: 0.0 }), ..fx() }),
        p("Warm edge", "grit", GildFx { grit: Some(GildGrit { drive: 0.45, flavour: GritFlavour::Warm, blend: 0.5, level_db: 0.0 }), ..fx() }),
        p("Megaphone bite", "grit", GildFx { grit: Some(GildGrit { drive: 0.8, flavour: GritFlavour::Hard, blend: 0.85, level_db: -2.0 }), ..fx() }),
        p("Tight double", "double", GildFx { double: Some(GildDouble { cents: 6.0, ms: 14.0, level_db: -7.0 }), ..fx() }),
        p("Wide double", "double", GildFx { double: Some(GildDouble { cents: 14.0, ms: 32.0, level_db: -5.0 }), ..fx() }),
        p("Quarter-note throw", "delay", GildFx { delay: Some(echo(GildNote::Quarter, 0.4, 220.0, 5200.0, false, -5.0)), ..fx() }),
        p("Dotted-eighth bounce", "delay", GildFx { delay: Some(echo(GildNote::DottedEighth, 0.45, 260.0, 4800.0, true, -6.0)), ..fx() }),
        p(
            "Slap on the word",
            "delay",
            GildFx {
                delay: Some(GildEcho { sync: false, ms: 110.0, repeats: 0.08, low_cut: 150.0, high_cut: 7000.0, ..GildEcho::default() }),
                ..fx()
            },
        ),
        p("Half-note fade", "delay", GildFx { delay: Some(echo(GildNote::Half, 0.6, 320.0, 3600.0, false, -8.0)), ..fx() }),
        p("Long hall throw", "reverb", GildFx { reverb: Some(verb(GildSpace::Hall, 0.85, 5.5, 60.0, 1.2, -6.0)), ..fx() }),
        p("Bright plate splash", "reverb", GildFx { reverb: Some(verb(GildSpace::Plate, 0.5, 1.8, 20.0, 1.0, -7.0)), ..fx() }),
        p("Small room glow", "reverb", GildFx { reverb: Some(verb(GildSpace::Room, 0.35, 0.9, 10.0, 0.8, -9.0)), ..fx() }),
        p("Cloud bloom", "reverb", GildFx { reverb: Some(verb(GildSpace::Cloud, 0.9, 8.0, 90.0, 1.4, -8.0)), ..fx() }),
        p(
            "Soft swell-in",
            "swell",
            GildFx { swell: Some(GildSwell { length_s: 1.2, offset_ms: 0.0, space: GildSpace::Hall, level_db: -6.0 }), ..fx() },
        ),
        p(
            "Long rise",
            "swell",
            GildFx { swell: Some(GildSwell { length_s: 3.0, offset_ms: -60.0, space: GildSpace::Cloud, level_db: -4.0 }), ..fx() },
        ),
        p(
            "Last-word throw",
            "gilding",
            GildFx {
                delay: Some(echo(GildNote::Quarter, 0.42, 240.0, 5000.0, false, -6.0)),
                reverb: Some(verb(GildSpace::Hall, 0.8, 4.2, 50.0, 1.1, -11.0)),
                ..fx()
            },
        ),
        p(
            "Swell into the word",
            "gilding",
            GildFx {
                swell: Some(GildSwell { length_s: 1.6, offset_ms: 0.0, space: GildSpace::Hall, level_db: -5.0 }),
                reverb: Some(verb(GildSpace::Hall, 0.6, 2.2, 30.0, 1.0, -12.0)),
                ..fx()
            },
        ),
        p(
            "Big ending",
            "gilding",
            GildFx {
                double: Some(GildDouble { cents: 10.0, ms: 24.0, level_db: -6.0 }),
                delay: Some(echo(GildNote::Half, 0.5, 280.0, 4200.0, true, -7.0)),
                reverb: Some(verb(GildSpace::Hall, 0.9, 6.5, 70.0, 1.3, -8.0)),
                ..fx()
            },
        ),
        p(
            "Spotlight word",
            "gilding",
            GildFx {
                double: Some(GildDouble { cents: 7.0, ms: 18.0, level_db: -8.0 }),
                reverb: Some(verb(GildSpace::Plate, 0.55, 2.0, 25.0, 1.0, -9.0)),
                ..fx()
            },
        ),
    ]
}

/// The preset named `name` (case aside).
pub fn find(name: &str) -> Option<GildPreset> {
    all().into_iter().find(|p| p.name.eq_ignore_ascii_case(name.trim()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn presets_are_in_range_and_unique() {
        let all = super::all();
        let mut names: Vec<&str> = all.iter().map(|p| p.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all.len());
        for p in all {
            let mut f = p.fx.clone();
            f.sanitize();
            assert_eq!(f, p.fx, "{} is within the ranges", p.name);
            assert!(!p.fx.is_empty());
            assert!(p.kind == "gilding" || p.fx.kinds() == vec![p.kind], "{}", p.name);
        }
    }
}
