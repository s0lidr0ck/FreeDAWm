//! `PedalConfig.h`'s switch descriptions: one line per option, shown under each switch row.
//! (Taken from the C++ table; the rows' labels and parameters are the dsp port's
//! `wrapper::CIRCUITS`.)

/// `(param id, [description per option])`.
pub(super) static DESCRIPTIONS: &[(&str, &[&str])] = &[
    (
        "tsModel",
        &[
            "The original. Warm, full low-mids. JRC4558, 100 Ω / 10 kΩ output.",
            "Brighter, slightly thinner than the 808. Same circuit, 470 Ω / 100 kΩ output.",
            "Later TS9s with the TA75558 chip. A touch more top end, slightly less headroom.",
            "Late-80s version. Same core as the TS9, usually a different chip.",
            "TS9DX hot mode. More gain, more bass, double diodes for a louder, grittier clip.",
            "Boutique-style mod. Asymmetric clipping, extra bass, 808 output stage.",
        ],
    ),
    (
        "tsDiode",
        &[
            "Stock — whatever the selected model shipped with.",
            "1N914 — stock silicon signal diode.",
            "1N4148 — practically identical to 1N914.",
            "1S1588 — original Japanese diode, marginally softer knee.",
            "Asymmetric — two diodes one way, one the other. Even harmonics, more amp-like.",
            "LED — ~1.7 V knee. Much louder and more open.",
            "Germanium — ~0.3 V knee. Compressed and dark.",
        ],
    ),
    (
        "ratModel",
        &[
            "Vintage RAT. LM308 chip, 1N914 silicon clippers. The slow slew rate of the LM308 is the sound.",
            "Modern RAT2. OP07 chip, 1N914 clippers. Set Chip to LM308 for an early RAT2.",
            "Turbo RAT. LM308 with red LED clippers (~1.7 V). Louder, more open, less compressed.",
            "You Dirty Rat. LM308 with germanium clippers (~0.3 V). Fuzzier, darker, more squashed.",
            "Fat RAT. OP07, 1N914 clippers, plus the Fat switch for more low end. Values approximate.",
        ],
    ),
    (
        "ratDiode",
        &[
            "Stock — whatever the selected model shipped with.",
            "1N914 — silicon, ~0.6 V knee. The classic RAT clip.",
            "Red LED — ~1.7 V knee. Turbo RAT territory.",
            "Germanium 1N34A — ~0.3 V knee. You Dirty Rat territory.",
            "MOSFET — ~1.8 V knee, slightly softer than LEDs. Fat RAT option (approx.).",
        ],
    ),
    (
        "ratChip",
        &[
            "Stock — whatever the selected model shipped with.",
            "LM308 — 1 MHz, 0.3 V/µs. The slow vintage chip.",
            "OP07 — 0.6 MHz, 0.3 V/µs. Modern RAT2 chip. Slightly narrower bandwidth.",
            "LM741 — 1 MHz, 0.5 V/µs. A little faster and brighter on attacks.",
            "TL071 — 3 MHz, 13 V/µs. A fast chip: hear what the slew limiting was doing.",
        ],
    ),
    ("ratFat", &["Fat off — stock 4.7 µF on the 47 Ω gain leg.", "Fat on — 22 µF on the 47 Ω gain leg: full gain reaches lower, more low end."]),
    (
        "muffVer",
        &[
            "Triangle (1970). Deep scoop, the most bass, a little wild.",
            "Ram's Head (1973). The Gilmour one. Slightly less bass, smoother sustain.",
            "Russian (Sovtek). More mids and low end, less fizz. Bass players' favourite.",
            "NYC reissue. Bright and scooped, the modern stock sound.",
        ],
    ),
    (
        "muffDiode",
        &[
            "Stock silicon in both clipping stages.",
            "1N914 — same as stock.",
            "LEDs — much more headroom per stage, louder and more open.",
            "Germanium — squashier, darker, more compression.",
        ],
    ),
    ("muffMids", &["Stock tone stack — the classic mid scoop.", "Mids mod — tone stack corners crossed so the scoop fills in. Sits in a mix."]),
    (
        "ffTrans",
        &[
            "Germanium (NKT275-style). Softer, darker, sags a little and cleans up with Input.",
            "Silicon (BC108-style). Harder, brighter, more gain, less cleanup.",
        ],
    ),
    (
        "ffBias",
        &[
            "Cold bias — gated, spitty, velcro-ish decay.",
            "Stock bias — the collector sitting where it should.",
            "Hot bias — smoother, more sustain, less sputter.",
        ],
    ),
    (
        "ds1Diode",
        &[
            "Stock 1N4148 pair. The classic buzzsaw.",
            "LEDs — louder and more open; a common mod.",
            "Asymmetric — three diodes, one way and two the other. Amp-like even harmonics.",
        ],
    ),
    (
        "ds1Voice",
        &[
            "Stock voicing — mid-scooped, fizzy top. The 90s sound.",
            "Modded — less scoop, less fizz, a little less pre-gain. The boutique-mod direction.",
        ],
    ),
    (
        "klonDiode",
        &[
            "Germanium 1N34A — stock. Low knee, the clean blend does the heavy lifting.",
            "Silicon — harder edge, closer to a TS at high gain.",
            "LED — big headroom, mostly a loud clean boost with hair.",
        ],
    ),
    ("bbGain", &["Stock Blues Breaker gain range. Low, transparent, two series diodes each way.", "Hi — King-of-Tone-style extra gain range."]),
    (
        "govClip",
        &[
            "Stock — TL072 gain stage into a pair of LEDs. Open, amp-like, lots of headroom.",
            "1N914s in the gain-stage loop as well, then the LEDs. Smoother, more compressed.",
        ],
    ),
    ("govBass", &["Bass shelf −7 dB at 150 Hz.", "Bass control at noon.", "Bass shelf +7 dB at 150 Hz."]),
    ("govMid", &["Mid −8 dB at 650 Hz. Scooped.", "Middle control at noon.", "Mid +8 dB at 650 Hz. The Marshall honk."]),
    (
        "tdPlate",
        &[
            "Starved plate at 9 V. The tube barely conducts — spongy, compressed, grid conduction on every attack.",
            "12 V, the stock Tube Driver supply. Still starved, a little more headroom and clarity.",
            "250 V — a proper amp preamp stage. Harder knee, far more headroom, closer to an amp's front end.",
        ],
    ),
    (
        "tdBias",
        &[
            "Cold — 4.7 k cathode. Earlier cutoff, more asymmetric, spittier.",
            "Stock — 1.5 k cathode.",
            "Hot — 680 Ω cathode. More current, smoother, more even-harmonic warmth.",
        ],
    ),
    (
        "bdMode",
        &[
            "Stock BD-2. JFET front end into the discrete gain block, 1N4148s in the loop.",
            "Keeley Phat mod — bigger gain-leg cap, full low end all the way up.",
        ],
    ),
    (
        "dpDiode",
        &[
            "1N270 germanium — stock. Soft, low knee, the Randy Rhoads sound.",
            "Silicon — the DOD 250 direction. Louder, harder.",
            "LED — big headroom, mostly a boost with hair.",
        ],
    ),
    ("ocdMode", &["HP — more gain and low end. The 'big amp' setting.", "LP — less gain, a touch less bass, extra top roll-off. Sits back."]),
];
