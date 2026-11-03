//! `CompConfig.h` / `CompConfig.cpp`: the styles table and `fs::comp::model`, the UI's own copy
//! of the DSP's parameter maps (the C++ editor keeps it too) for the readouts, the knobs a style
//! hides or steps, its switches, and the static transfer curve.

/// `StyleDef` (the accent is unused: one accent per engine).
pub struct StyleDef {
    pub name: &'static str,
    pub tag: &'static str,
    pub blurb: &'static str,
    /// The hardware credit line (`crate::hardware_credit`); empty for our own designs.
    pub credit: &'static str,
}

/// `getStyles()`.
pub const STYLES: [StyleDef; 10] = [
    StyleDef { name: "Clean", tag: "Modern", blurb: "Transparent feed-forward. Every control is live.", credit: "" },
    StyleDef {
        name: "1176",
        tag: "FET",
        blurb: "From the UREI drawings: FET attenuator, the ratio switch's real taps and bias voltages, 0.22 \u{b5}F timing cap with the Attack and Release pots. Blue stripe = Rev A, Blackface = 1176LN. All buttons is simply every switch closed.",
        credit: "Component-level model of the UREI 1176. No affiliation; UREI and 1176 are trademarks of their owner.",
    },
    StyleDef {
        name: "LA-2A",
        tag: "Opto",
        blurb: "From the Teletronix drawing: 68k into the T4 cell, sidechain fed from above R7. Limit only leaves R7 in circuit, so it is a subtle change, as on the real unit. Emphasis is the R37 trimmer.",
        credit: "Component-level model of the Teletronix LA-2A. No affiliation; Teletronix and LA-2A are trademarks of their owner.",
    },
    StyleDef {
        name: "160",
        tag: "VCA",
        blurb: "True-RMS feed-forward. Timing sets itself; release falls at a constant dB per second.",
        credit: "Modelled after the dbx 160. No affiliation; dbx is a trademark of its owner.",
    },
    StyleDef {
        name: "G Bus",
        tag: "SSL",
        blurb: "Circuit model, component for component: sidechain VCA that mirrors the audio VCAs, peak rectifier, the ratio stage's resistors (and the 2:1 diode network), attack resistors, timing caps, Auto network.",
        credit: "Component-level model of the SSL G Series bus compressor. No affiliation; SSL is a trademark of its owner.",
    },
    StyleDef {
        name: "Vari-Mu",
        tag: "670",
        blurb: "Against the Fairchild drawing: feedback sidechain, bridge rectifier against the DC threshold (so the ratio keeps climbing), and the real timing parts: 220k with 2, 4, 8 or 20 \u{b5}F, plus 100k memory branches on positions 5 and 6.",
        credit: "Component-level model of the Fairchild 670. No affiliation; Fairchild is a trademark of its owner.",
    },
    StyleDef {
        name: "Ross",
        tag: "Pedal",
        blurb: "From the Ross drawing: treble pre-emphasis into the CA3080 (so pick attack distorts first), charge-pump rectifiers, 10 \u{b5}F envelope cap, linear Sustain pot. Attack at Stock and Release at 150 k is the pedal as built. The Dyna Comp is the same circuit with lower-gain transistors.",
        credit: "Component-level model of the Ross Compressor. No affiliation; Ross and Dyna Comp are trademarks of their owners.",
    },
    StyleDef {
        name: "EL8",
        tag: "Distress",
        blurb: "Eight ratios with their own knees (10:1 is the opto one), Dist 2 and Dist 3, detector HP and band emphasis.",
        credit: "Modelled after the Empirical Labs Distressor (EL8). No affiliation; Empirical Labs and Distressor are trademarks of their owner.",
    },
    StyleDef {
        name: "2500",
        tag: "Thrust",
        blurb: "RMS bus compressor. Old is feedback, New is feed-forward; Thrust tilts the sidechain so the low end stops driving it.",
        credit: "Modelled after the API 2500. No affiliation; API is a trademark of its owner.",
    },
    StyleDef {
        name: "33609",
        tag: "Diode",
        blurb: "From the Neve card drawing and technical handbook: diode bridge solved per sample, 10 \u{b5}F timing cap, the recovery switch's real resistors pulling towards a reference (so recovery is nearly a straight line in dB), and the two Auto networks. Ratios pass the factory test.",
        credit: "Component-level model of the Neve 33609. No affiliation; Neve is a trademark of its owner.",
    },
];

/// `channelModeName`.
pub fn channel_mode_name(i: i32) -> &'static str {
    ["Stereo", "Mid / Side", "Mid only", "Side only"][i.clamp(0, 3) as usize]
}

/// `CompValues`: what the readouts and the curve depend on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompValues {
    pub style: i32,
    pub amount: f32,
    pub ratio: f32,
    pub attack: f32,
    pub release: f32,
    pub makeup: f32,
    pub knee: f32,
    pub sc_hpf: f32,
    pub color: f32,
    pub link: f32,
    pub variant: i32,
    pub det_mode: i32,
    pub chan_mode: i32,
}

impl Default for CompValues {
    fn default() -> Self {
        CompValues {
            style: 0,
            amount: 0.4,
            ratio: 0.4,
            attack: 0.5,
            release: 0.5,
            makeup: 0.3333,
            knee: 0.25,
            sc_hpf: 0.0,
            color: 0.5,
            link: 1.0,
            variant: 0,
            det_mode: 0,
            chan_mode: 0,
        }
    }
}

impl CompValues {
    /// A key for caching what depends on these values.
    pub fn key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for v in [self.amount, self.ratio, self.attack, self.release, self.makeup, self.knee, self.sc_hpf, self.color, self.link] {
            v.to_bits().hash(&mut h);
        }
        (self.style, self.variant, self.det_mode, self.chan_mode).hash(&mut h);
        h.finish()
    }
}

fn clampf(v: f32, a: f32, b: f32) -> f32 {
    if v < a {
        a
    } else if v > b {
        b
    } else {
        v
    }
}

/// `juce::roundToInt`: half away from zero (as `std::lround` for these positive values).
fn round_to_int(x: f32) -> i32 {
    if x.is_finite() { x.round() as i32 } else { 0 }
}

/// `idx (v, n)`: the switch position of a stepped knob.
pub fn idx(v: f32, n: i32) -> usize {
    round_to_int(clampf(v, 0.0, 1.0) * (n - 1) as f32).clamp(0, (n - 1).max(0)) as usize
}

/// JS `Infinity`.
pub const K_INF: f32 = 1.0e30;

pub fn ratio_from_knob(t: f32) -> f32 {
    if t > 0.965 {
        return K_INF;
    }
    let u = t / 0.965;
    1.0 + 19.0 * u * u
}

/// `steps`: how many positions a knob has for this style (0: continuous).
pub fn steps(id: &str, style: i32) -> i32 {
    match id {
        "ratio" => match style {
            1 => 5,
            2 => 2,
            4 => 3,
            7 => 8,
            8 => 7,
            9 => 6,
            _ => 0,
        },
        "attack" => match style {
            4 => 6,
            8 => 7,
            9 => 2,
            _ => 0,
        },
        "release" => match style {
            4 => 5,
            5 | 9 => 6,
            _ => 0,
        },
        "knee" => match style {
            3 => 2,
            8 => 3,
            _ => 0,
        },
        _ => 0,
    }
}

/// `visible`.
pub fn visible(id: &str, style: i32) -> bool {
    match id {
        "ratio" => style != 5 && style != 6,
        "attack" => style != 2 && style != 3 && style != 5,
        "release" => style != 2 && style != 3,
        "knee" => style == 0 || style == 3 || style == 8,
        "color" => style != 0,
        _ => true,
    }
}

/// `switchDef`: the switch's caption and positions (empty when the style does not use it).
pub fn switch_def(id: &str, style: i32) -> (&'static str, &'static [&'static str]) {
    match (id, style) {
        ("variant", 1) => ("Revision", &["Blue stripe", "Blackface"]),
        ("variant", 6) => ("Circuit", &["Ross", "Dyna Comp"]),
        ("variant", 7) => ("Audio", &["Clean", "Dist 2", "Dist 3"]),
        ("variant", 8) => ("Thrust", &["Norm", "Med", "Loud"]),
        ("detMode", 2) => ("Emphasis", &["Flat", "HF"]),
        ("detMode", 7) => ("Detector", &["Norm", "HP", "Band", "HP + Band"]),
        ("detMode", 8) => ("Type", &["New", "Old"]),
        _ => ("", &[]),
    }
}

/// `label`.
pub fn label(id: &str, style: i32) -> &'static str {
    const AMOUNT: [&str; 10] =
        ["Threshold", "Input", "Peak reduction", "Threshold", "Threshold", "Threshold", "Sustain", "Threshold", "Threshold", "Threshold"];
    const MAKEUP: [&str; 10] = ["Makeup", "Output", "Gain", "Output", "Makeup", "Output", "Level", "Output", "Makeup", "Gain"];
    let s = style.clamp(0, 9) as usize;
    match id {
        "amount" => AMOUNT[s],
        "ratio" => {
            if s == 2 {
                "Mode"
            } else {
                "Ratio"
            }
        }
        "attack" => "Attack",
        "release" => {
            if s == 5 {
                "Time constant"
            } else {
                "Release"
            }
        }
        "makeup" => MAKEUP[s],
        "knee" => "Knee",
        "scHpf" => "Sidechain HPF",
        "color" => "Color",
        "link" => "Stereo link",
        _ => "",
    }
}

/// 500k linear Sustain pot + 27k.
pub fn ross_gain(a: f32) -> f32 {
    97.2 * 27000.0 / (27000.0 + 500000.0 * (1.0 - clampf(a, 0.0, 1.0)))
}

pub fn threshold(v: &CompValues) -> f32 {
    let s = v.style;
    let a = v.amount;
    if s == 5 {
        return -45.0 * a;
    }
    if s == 6 {
        return 20.0 * (0.555 / (1.5 * ross_gain(a))).log10();
    }
    let mut t = -50.0 * a;
    if s == 1 {
        const BLUE: [f32; 5] = [-4.0, 7.0, 10.0, 16.0, -12.0];
        const BLACK: [f32; 5] = [-1.0, 0.0, 1.0, 4.0, -7.0];
        t += (if v.variant < 1 { BLUE } else { BLACK })[idx(v.ratio, 5)];
    }
    t
}

pub fn ratio_of(v: &CompValues) -> f32 {
    match v.style {
        0 | 3 => ratio_from_knob(v.ratio),
        4 => [2.0, 4.0, 10.0][idx(v.ratio, 3)],
        7 => [1.0, 2.0, 3.0, 4.0, 6.0, 10.0, 20.0, 100.0][idx(v.ratio, 8)],
        8 => [1.5, 2.0, 3.0, 4.0, 6.0, 10.0, K_INF][idx(v.ratio, 7)],
        9 => [1.5, 2.0, 3.0, 4.0, 6.0, 121.0][idx(v.ratio, 6)],
        _ => 100.0,
    }
}

pub fn knee_of(v: &CompValues) -> f32 {
    match v.style {
        0 => 24.0 * v.knee,
        3 => {
            if v.knee >= 0.5 {
                14.0
            } else {
                0.0
            }
        }
        4 => [12.0, 8.0, 4.0][idx(v.ratio, 3)],
        7 => [0.0, 30.0, 20.0, 15.0, 10.0, 20.0, 6.0, 2.0][idx(v.ratio, 8)],
        8 => [0.0, 6.0, 14.0][idx(v.knee, 3)],
        9 => {
            if idx(v.ratio, 6) == 5 {
                2.0
            } else {
                6.0
            }
        }
        _ => 3.0,
    }
}

pub fn knee_fn(o: f32, w: f32) -> f32 {
    if w < 0.01 {
        return o.max(0.0);
    }
    if o <= -0.5 * w {
        return 0.0;
    }
    if o >= 0.5 * w {
        return o;
    }
    let t = o + 0.5 * w;
    t * t / (2.0 * w)
}

/// Feedback circuits: find y with y = x * gain(y). Bisection, as in the JS.
fn solve_feedback(x: f32, gain_for: impl Fn(f32) -> f32) -> f32 {
    let (mut lo, mut hi) = (0.0f32, x);
    for _ in 0..40 {
        let y = 0.5 * (lo + hi);
        if y > x * gain_for(y) {
            hi = y;
        } else {
            lo = y;
        }
    }
    20.0 * (0.5 * (lo + hi)).max(1.0e-9).log10()
}

/// `staticOut`: the steady-state output level (dB) for an input level (dB), before makeup.
pub fn static_out(v: &CompValues, lev_in: f32) -> f32 {
    let s = v.style;
    let x = 10f32.powf(lev_in / 20.0);

    if s == 1 {
        // 1176: ratio switch taps + bias voltages from the UREI drawings
        let i = idx(v.ratio, 5);
        let blue = v.variant < 1;
        let in_g = 10f32.powf(2.5 * v.amount);
        const BLUE_TAP: [f32; 5] = [0.383, 0.446, 0.536, 1.0, 1.0];
        const BLACK_TAP: [f32; 5] = [0.166, 0.364, 0.562, 0.802, 0.456];
        const BLUE_VA: [f32; 5] = [0.0, -0.971, -1.942, -7.521, 0.0];
        const BLACK_VA: [f32; 5] = [-0.307, -1.270, -2.418, -5.492, -0.638];
        let tap = if blue { BLUE_TAP[i] } else { BLACK_TAP[i] };
        let va = if blue { BLUE_VA[i] } else { BLACK_VA[i] };
        let knee = 0.047 * (0.047 / (270.0e3 * 19.5f32.powf(v.release) * 5.0e-12)).ln() + 0.1;
        let a = (if blue { 1.54 } else { 5.42 }) * tap * in_g;
        let vth = -0.19 - va + knee;
        return solve_feedback(x, |y| 1.0 / (1.0 + 17.5 * ((a * y - vth).max(0.0) - 0.03).max(0.0)));
    }

    if s == 4 {
        // G Bus: sidechain VCA loop, values from the schematic
        let i = idx(v.ratio, 3);
        const CS: [f32; 3] = [0.6542, 2.2517, 10.0];
        let c = CS[i];
        let t = 10f32.powf(threshold(v) / 20.0);
        let ag = (c + 0.14327) * 1.122_018_5 / t;
        return solve_feedback(x, |y| {
            let raw = ag * y;
            let mut u = raw - c;
            if i == 0 {
                let l2 = 0.255 * raw + 1.06;
                let d = (u - l2) / 0.08;
                u = u.min(l2) - 0.08 * (1.0 + (-d.abs()).exp()).ln();
            }
            (-0.8036 * clampf(u, 0.0, 9.9)).exp()
        });
    }

    if s == 2 {
        // LA-2A: panel drive -> light -> cell conductance
        let in_g = 10f32.powf(2.5 * v.amount) * 0.62;
        let ff = if v.ratio >= 0.5 { 0.045 } else { 0.0 };
        return solve_feedback(x, |y| 1.0 / (1.0 + (0.8 * (y + ff * x) * in_g - 0.1).max(0.0).powf(2.7)));
    }

    let thr = threshold(v);
    let r = ratio_of(v);
    let w = knee_of(v);

    if s == 5 {
        // 670: linear rectifier past a DC threshold, control volts are dB
        let t = 10f32.powf(thr / 20.0);
        return solve_feedback(x, |y| {
            let z = (y / t - 1.0) / 0.08;
            let sp = if z > 18.0 {
                z
            } else if z < -6.0 {
                0.0
            } else {
                (1.0 + z.exp()).ln()
            };
            10f32.powf(-25.0 * 0.08 * sp / 20.0)
        });
    }

    let fb = s == 9 || (s == 8 && v.det_mode >= 1);
    let wanted = |lev: f32| {
        let o = lev - thr;
        let slope = if fb { (r - 1.0).min(60.0) } else { 1.0 - 1.0 / r };
        -slope * knee_fn(o, w)
    };
    // RMS detectors read a sine 3 dB under its peak
    let rms = if s == 3 || s == 8 { 3.0 } else { 0.0 };
    if !fb {
        return lev_in + wanted(lev_in - rms);
    }
    let (mut lo, mut hi) = (-120.0f32, 60.0f32);
    for _ in 0..28 {
        let m = 0.5 * (lo + hi);
        if m - wanted(m - rms) > lev_in {
            hi = m;
        } else {
            lo = m;
        }
    }
    0.5 * (lo + hi)
}

/// `msText`.
pub fn ms_text(x: f32) -> String {
    if x < 1.0 {
        format!("{} \u{b5}s", round_to_int(x * 1000.0))
    } else if x < 10.0 {
        format!("{x:.1} ms")
    } else if x < 1000.0 {
        format!("{} ms", round_to_int(x))
    } else if x < 10000.0 {
        format!("{:.2} s", x / 1000.0)
    } else {
        format!("{:.1} s", x / 1000.0)
    }
}

/// `juce::String (float, decimals)`.
fn fixed(x: f32, d: usize) -> String {
    format!("{x:.d$}")
}

/// `format`: the readout under a knob, in real units.
pub fn format(id: &str, v: &CompValues) -> String {
    let s = v.style;
    let pick = |opts: &[&str], n: i32, x: f32| opts.get(idx(x, n)).copied().unwrap_or("").to_string();
    match id {
        "amount" => match s {
            1 => format!("+{} dB", fixed(50.0 * v.amount, 1)),
            2 => round_to_int(v.amount * 100.0).to_string(),
            6 => fixed(v.amount * 10.0, 1),
            _ => format!("{} dB", fixed(threshold(v), 1)),
        },
        "ratio" => {
            let x = v.ratio;
            match s {
                1 => pick(&["4:1", "8:1", "12:1", "20:1", "All"], 5, x),
                2 => (if x >= 0.5 { "Limit" } else { "Compress" }).to_string(),
                4 => pick(&["2:1", "4:1", "10:1"], 3, x),
                7 => pick(&["1:1", "2:1", "3:1", "4:1", "6:1", "10:1 Opto", "20:1", "Nuke"], 8, x),
                8 => pick(&["1.5:1", "2:1", "3:1", "4:1", "6:1", "10:1", "\u{221e}:1"], 7, x),
                9 => pick(&["1.5:1", "2:1", "3:1", "4:1", "6:1", "Limit"], 6, x),
                _ => {
                    let r = ratio_from_knob(x);
                    if r >= K_INF { "\u{221e}:1".to_string() } else { format!("{}:1", fixed(r, if r < 10.0 { 1 } else { 0 })) }
                }
            }
        }
        "attack" => {
            let x = v.attack;
            match s {
                0 => ms_text(0.05 * 5000f32.powf(x)),
                1 => ms_text(570.0 * 45f32.powf(x) * 0.22e-3 / 7.0),
                4 => pick(&["0.1 ms", "0.3 ms", "1 ms", "3 ms", "10 ms", "30 ms"], 6, x),
                6 => {
                    let r = 20000.0 * x * x * x;
                    if r < 1.0 {
                        "Stock".to_string()
                    } else if r < 1000.0 {
                        format!("{} \u{3a9}", round_to_int(r))
                    } else {
                        format!("{} k\u{3a9}", fixed(r / 1000.0, 1))
                    }
                }
                7 => ms_text(0.05 * 600f32.powf(x)),
                8 => pick(&["0.03 ms", "0.1 ms", "0.3 ms", "1 ms", "3 ms", "10 ms", "30 ms"], 7, x),
                9 => {
                    let lim = idx(v.ratio, 6) == 5;
                    if lim {
                        (if x >= 0.5 { "Slow \u{b7} 4 ms" } else { "Fast \u{b7} 2 ms" }).to_string()
                    } else {
                        (if x >= 0.5 { "Stock" } else { "Fast (/N)" }).to_string()
                    }
                }
                _ => ms_text(50f32.powf(x)),
            }
        }
        "release" => {
            let x = v.release;
            match s {
                0 => ms_text(10.0 * 250f32.powf(x)),
                1 => ms_text(270.0e3 * 19.5f32.powf(x) * 0.22e-3),
                4 => pick(&["0.1 s", "0.3 s", "0.6 s", "1.2 s", "Auto"], 5, x),
                5 => pick(&["1 \u{b7} 0.3 s", "2 \u{b7} 0.9 s", "3 \u{b7} 1.8 s", "4 \u{b7} 4.4 s", "5 \u{b7} Auto", "6 \u{b7} Auto fast"], 6, x),
                6 => {
                    let r = 15.0 * 100f32.powf(x);
                    if r < 1000.0 { format!("{} k\u{3a9}", round_to_int(r)) } else { format!("{} M\u{3a9}", fixed(r / 1000.0, 1)) }
                }
                7 => ms_text(50.0 * 70f32.powf(x)),
                8 => ms_text(50.0 * 60f32.powf(x)),
                9 => pick(&["100 ms", "400 ms", "800 ms", "1.5 s", "Auto 1", "Auto 2"], 6, x),
                _ => ms_text(100.0 * 20f32.powf(x)),
            }
        }
        "makeup" => {
            let d = -12.0 + 36.0 * v.makeup;
            if d.abs() < 0.05 { "0.0 dB".to_string() } else { format!("{}{} dB", if d > 0.0 { "+" } else { "" }, fixed(d, 1)) }
        }
        "knee" => match s {
            3 => (if v.knee >= 0.5 { "OverEasy" } else { "Hard" }).to_string(),
            8 => pick(&["Hard", "Med", "Soft"], 3, v.knee),
            _ => format!("{} dB", fixed(24.0 * v.knee, 1)),
        },
        "scHpf" => {
            if v.sc_hpf <= 0.02 {
                "Off".to_string()
            } else {
                format!("{} Hz", round_to_int(20.0 * 25f32.powf(v.sc_hpf)))
            }
        }
        "color" => fixed(v.color * 10.0, 1),
        "link" => format!("{} %", round_to_int(v.link * 100.0)),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readouts_match_the_screenshot_defaults() {
        let v = CompValues { amount: 0.4, ratio: 0.4, attack: 0.5, release: 0.5, makeup: 0.3333, knee: 0.25, ..CompValues::default() };
        assert_eq!(format("amount", &v), "-20.0 dB");
        assert_eq!(format("ratio", &v), "4.3:1");
        assert_eq!(format("attack", &v), "3.5 ms");
        assert_eq!(format("release", &v), "158 ms");
        assert_eq!(format("makeup", &v), "0.0 dB");
        assert_eq!(format("knee", &v), "6.0 dB");
        assert_eq!(format("scHpf", &v), "Off");
        assert_eq!(format("link", &v), "100 %");
    }

    #[test]
    fn the_curve_is_finite_for_every_style() {
        for style in 0..10 {
            for variant in 0..3 {
                for det_mode in 0..4 {
                    let v = CompValues { style, variant, det_mode, ratio: 0.7, amount: 0.8, ..CompValues::default() };
                    for i in 0..=120 {
                        let lev = -60.0 + i as f32 * 0.5;
                        assert!(static_out(&v, lev).is_finite(), "style {style} at {lev}");
                    }
                }
            }
        }
        let clean = CompValues::default();
        assert!((static_out(&clean, -40.0) + 40.0).abs() < 1e-4, "below the threshold the curve is unity");
        assert!(static_out(&clean, 0.0) < -10.0, "above it, compressed");
    }
}
