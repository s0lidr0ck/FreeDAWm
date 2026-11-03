//! Port of `Presets.h` (GENERATED in MS by Tools/export-library.js from tables.js presets).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgressionPreset {
    pub name: &'static str,
    pub scale: i32,
    pub bars: i32,
    pub slots: [i32; 8],
}

const fn p(name: &'static str, scale: i32, bars: i32, slots: [i32; 8]) -> ProgressionPreset {
    ProgressionPreset { name, scale, bars, slots }
}

/// Index = progression parameter value - 1 (0 = Original, 17 = Custom).
pub const PROGRESSION_PRESETS: [ProgressionPreset; 16] = [
    p("Axis · I–V–vi–IV", 0, 4, [0, 64, 80, 48, 0, 64, 80, 48]),
    p("Doo-wop · I–vi–IV–V", 0, 4, [0, 80, 48, 64, 0, 80, 48, 64]),
    p("Sensitive · vi–IV–I–V", 0, 4, [80, 48, 0, 64, 80, 48, 0, 64]),
    p("Pop Lift · I–IV–vi–V", 0, 4, [0, 48, 80, 64, 0, 48, 80, 64]),
    p("Royal Road · IV–V–iii–vi", 0, 4, [49, 66, 33, 80, 49, 66, 33, 80]),
    p("Canon · 8 bars", 0, 8, [0, 64, 80, 32, 48, 0, 48, 64]),
    p("Andalusian · i–♭VII–♭VI–V", 1, 4, [0, 96, 80, 71, 0, 96, 80, 71]),
    p("Epic · i–♭VI–♭III–♭VII", 1, 4, [0, 80, 32, 96, 0, 80, 32, 96]),
    p("Minor Drift · i–iv–♭VII–♭III", 1, 4, [0, 48, 96, 32, 0, 48, 96, 32]),
    p("Mixolydian · I–♭VII–IV–I", 0, 4, [0, 103, 48, 0, 0, 103, 48, 0]),
    p("Worship · 8 bars", 0, 8, [0, 64, 80, 48, 0, 64, 48, 68]),
    p("Plagal · I–IV–I–IV", 0, 4, [0, 53, 0, 53, 0, 53, 0, 53]),
    p("Gospel · I–IV–I–V7", 0, 4, [0, 48, 0, 66, 0, 48, 0, 66]),
    p("Jazz · ii7–V7–Imaj7", 0, 4, [17, 66, 1, 1, 17, 66, 1, 1]),
    p("Neo-soul · IVmaj7–iii7–vi7–Vsus", 0, 4, [49, 33, 81, 68, 49, 33, 81, 68]),
    p("8-bar Blues", 0, 8, [2, 2, 50, 50, 2, 66, 2, 66]),
];
