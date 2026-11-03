//! FilterConfig -- the CIRCUITS table from FilterEngineUI.js, as data: what the two main knobs are
//! called on each circuit, the line it says about itself, its switch rows, how every value prints
//! and which modulation controls apply. Translation of `Engine 5 - Filter/Source/FilterConfig.h`.
//!
//! The knob -> unit maps are NOT repeated here. They are the engine's own
//! ([`super::filter_engine_dsp`]), so a readout cannot drift away from what the DSP does.
//!
//! Translation notes: `juce::String (x, n)` is `format!("{x:.n$}")`; `juce::roundToInt` is
//! `f32::round` then the cast (they differ only on an exact half, a display one unit off at
//! worst). `signedDb` prints a value that rounds to zero as `+0.0 dB`, as JUCE does for the tiny
//! negative values the trim knob's default gives.

use super::filter_engine_dsp::{self as d};

/// One switch row of a circuit: the parameter, its label, one description per option.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SwitchDef {
    pub param: usize,
    pub label: &'static str,
    pub descriptions: &'static [&'static str],
}

/// One circuit, in the order of the Circuit parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CircuitDef {
    pub name: &'static str,
    /// The real box, shown beside the name.
    pub full: &'static str,
    /// What Cutoff is called on this circuit.
    pub dial_cutoff: &'static str,
    pub dial_reso: &'static str,
    pub blurb: &'static str,
    pub switches: &'static [SwitchDef],
}

pub const CIRCUITS: [CircuitDef; 9] = [
    CircuitDef {
        name: "Q-Tron",
        full: "EHX Q-Tron+",
        dial_cutoff: "Sweep",
        dial_reso: "Peak",
        blurb: "Op-amp state-variable filter tuned by a photocell. 18 V rail, so it stays clean and wide. Values from the New Sensor drawing.",
        switches: &[
            SwitchDef {
                param: d::P_QT_RANGE,
                label: "Range",
                descriptions: &[
                    "Lo \u{2014} 1.8n + 2.2n integrator caps. Sweeps roughly 190 Hz to 2 kHz. Bass, baritone, thick chords.",
                    "Hi \u{2014} 1.8n caps. Sweeps roughly 420 Hz to 4.4 kHz (the factory alignment point). Guitar.",
                ],
            },
            SwitchDef {
                param: d::P_QT_MODE,
                label: "Mode",
                descriptions: &[
                    "Lowpass \u{2014} full low end, the sweep opens the top. The fat one.",
                    "Bandpass \u{2014} the quack. Thinner, vocal, cuts through.",
                    "Highpass \u{2014} the sweep removes lows as it rises. Glassy.",
                    "Mix \u{2014} lowpass and bandpass summed through 100k / 100k, as on the pedal.",
                ],
            },
        ],
    },
    CircuitDef {
        name: "Mu-Tron",
        full: "Mu-Tron III",
        dial_cutoff: "Sweep",
        dial_reso: "Peak",
        blurb: "The original. Same topology on a lower supply with a slower photocell: rounder attack, longer tail, grits up sooner. Values from recollection.",
        switches: &[
            SwitchDef {
                param: d::P_MU_RANGE,
                label: "Range",
                descriptions: &[
                    "Lo \u{2014} bigger integrator caps. About 60 Hz to 1.1 kHz. The Bootsy setting.",
                    "Hi \u{2014} about 190 Hz to 3.6 kHz. Guitar and clav.",
                ],
            },
            SwitchDef {
                param: d::P_MU_MODE,
                label: "Mode",
                descriptions: &[
                    "Lowpass \u{2014} fat and round.",
                    "Bandpass \u{2014} the Garcia / Stevie clav voice.",
                    "Highpass \u{2014} thin and bright, opens from the bottom.",
                ],
            },
        ],
    },
    CircuitDef {
        name: "Cry Baby",
        full: "Dunlop Cry Baby GCB-95",
        dial_cutoff: "Pedal",
        dial_reso: "Q",
        blurb: "Inductor wah solved at the component level: Q1 gain stage, 500 mH tank, 10n cap multiplied through the pot. Q is the resistor across the inductor (33k at noon).",
        switches: &[
            SwitchDef {
                param: d::P_CB_INDUCTOR,
                label: "Inductor",
                descriptions: &[
                    "Red Fasel \u{2014} low DC resistance, sharp peak, saturates late. The modern standard.",
                    "Yellow Fasel \u{2014} a touch more inductance and earlier saturation. Sweeter, a little lower.",
                    "Halo \u{2014} lossier core that holds some magnetism: slightly asymmetric, growly when pushed.",
                    "Stock TDK can \u{2014} highest DC resistance: softer peak in the heel, cleanest core.",
                ],
            },
            SwitchDef {
                param: d::P_CB_VOICE,
                label: "Sweep",
                descriptions: &[
                    "Stock 10n sweep cap \u{2014} about 380 Hz to 2.1 kHz.",
                    "Deep \u{2014} 22n. The whole sweep drops about an octave. Bass and baritone.",
                    "Vocal \u{2014} 6.8n. Higher, more nasal, more 'ow' than 'wah'.",
                ],
            },
        ],
    },
    CircuitDef {
        name: "Vox",
        full: "Vox V846 / Clyde McCoy",
        dial_cutoff: "Pedal",
        dial_reso: "Q",
        blurb: "The Vox values in the same solve: bigger emitter resistor and lower-gain transistors, so less gain, a higher heel and a more vocal throat.",
        switches: &[SwitchDef {
            param: d::P_VOX_VOICE,
            label: "Model",
            descriptions: &[
                "V846 \u{2014} 510 ohm emitter, late-60s production. Hendrix, Clapton.",
                "Clyde McCoy \u{2014} Halo inductor, lower-gain transistors. Rounder and throatier.",
            ],
        }],
    },
    CircuitDef {
        name: "Moog",
        full: "Moog transistor ladder",
        dial_cutoff: "Cutoff",
        dial_reso: "Emphasis",
        blurb: "Four differential-pair stages, each one a tanh, solved implicitly. Self-oscillates at full Emphasis. Loses bass as Emphasis rises, like the real one.",
        switches: &[
            SwitchDef {
                param: d::P_MOOG_SLOPE,
                label: "Slope",
                descriptions: &[
                    "24 dB \u{2014} output from the fourth stage. The Minimoog.",
                    "18 dB \u{2014} tap after three stages.",
                    "12 dB \u{2014} tap after two stages. Gentler, still resonant.",
                    "6 dB \u{2014} one stage. A warm tilt with the ladder's saturation.",
                ],
            },
            SwitchDef {
                param: d::P_MOOG_COMP,
                label: "Bass",
                descriptions: &[
                    "Stock \u{2014} passband drops as Emphasis comes up. Authentic.",
                    "Comp \u{2014} input gain follows Emphasis so the low end stays put.",
                ],
            },
        ],
    },
    CircuitDef {
        name: "MS-20",
        full: "Korg MS-20 (Korg35)",
        dial_cutoff: "Cutoff",
        dial_reso: "Peak",
        blurb: "Sallen-Key filter with a diode limiter inside the feedback loop. Loud input chokes the resonance; quiet input lets it scream. Self-oscillates.",
        switches: &[SwitchDef {
            param: d::P_MS_MODE,
            label: "Mode",
            descriptions: &[
                "Lowpass \u{2014} 12 dB per octave.",
                "Highpass \u{2014} 6 dB per octave, as on the MS-20.",
                "Both in series, highpass two octaves under the lowpass. The full MS-20 signal path.",
            ],
        }],
    },
    CircuitDef {
        name: "SEM",
        full: "Oberheim SEM",
        dial_cutoff: "Freq",
        dial_reso: "Res",
        blurb: "12 dB OTA state-variable. Smooth, wide, never self-oscillates. The polite one \u{2014} great on pads and whole mixes.",
        switches: &[SwitchDef {
            param: d::P_SEM_MODE,
            label: "Mode",
            descriptions: &["Lowpass.", "Bandpass.", "Highpass.", "Notch \u{2014} lowpass plus highpass. Sweep it slowly for a phaser-like hollow."],
        }],
    },
    CircuitDef {
        name: "TB-303",
        full: "Roland TB-303 diode ladder",
        dial_cutoff: "Cutoff",
        dial_reso: "Reso",
        blurb: "Diode ladder with a half-size first cap and stages that load each other. Rubbery. AC-coupled feedback, so the resonance thins out down low.",
        switches: &[SwitchDef {
            param: d::P_TB_MOD,
            label: "Mod",
            descriptions: &[
                "Stock \u{2014} 70 Hz to 6 kHz, resonance stops just short of oscillation.",
                "Devil Fish \u{2014} wider sweep and enough feedback to self-oscillate.",
            ],
        }],
    },
    CircuitDef {
        name: "Wasp",
        full: "EDP Wasp",
        dial_cutoff: "Freq",
        dial_reso: "Q",
        blurb: "State-variable filter built from CMOS logic inverters. They saturate unevenly and the Q path distorts: gritty and alive. Push Drive.",
        switches: &[SwitchDef { param: d::P_WASP_MODE, label: "Mode", descriptions: &["Lowpass.", "Bandpass.", "Highpass.", "Notch."] }],
    },
];

/// The circuit at `index`, clamped into the table.
pub fn circuit(index: i32) -> &'static CircuitDef {
    &CIRCUITS[index.clamp(0, 8) as usize]
}

/// `kNotes`: the cycle / step lengths. Must stay in step with `note_to_beats`.
pub const NOTES: [&str; 18] = [
    "4 Bars", "2 Bars", "1/1 D", "1/1", "1/2 D", "1/1 T", "1/2", "1/4 D", "1/2 T", "1/4", "1/8 D", "1/4 T", "1/8", "1/16 D", "1/8 T", "1/16",
    "1/16 T", "1/32",
];

/// `kStepCounts`: Steps is a stepped 0..14 parameter that the DSP reads as count - 2.
pub const STEP_COUNTS: [&str; 15] = ["2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16"];

// ---------------------------------------------------------------- value readouts
fn signed_db(db: f32) -> String {
    let s = format!("{db:.1}");
    if s == "-0.0" {
        return "+0.0 dB".to_string();
    }
    format!("{}{s} dB", if db >= 0.0 { "+" } else { "" })
}

fn seconds(s: f32) -> String {
    if s >= 1.0 { format!("{s:.2} s") } else { format!("{} ms", (s * 1000.0).round() as i64) }
}

/// How a parameter prints under its knob. `bpm` is the tempo actually in force (host or manual).
pub fn format_value(index: usize, v: f32, bpm: f32) -> String {
    match index {
        d::P_LEVEL => signed_db(d::knob_to_level_db(v)),
        d::P_TRIM => signed_db(d::knob_to_trim_db(v)),
        d::P_DRIVE => {
            if v <= 0.001 {
                "Off".to_string()
            } else {
                signed_db(d::knob_to_drive_db(v))
            }
        }
        d::P_MIX => {
            if v > 0.995 {
                "Wet only".to_string()
            } else {
                format!("{} %", (v * 100.0).round() as i64)
            }
        }
        d::P_DEPTH => {
            // bipolar: 0.5 is none, below that the sweep runs downwards
            let b = d::knob_to_depth_bipolar(v);
            if b.abs() < 0.005 {
                return "None".to_string();
            }
            format!("{}{} %", if b > 0.0 { "Up " } else { "Down " }, (b.abs() * 100.0).round() as i64)
        }
        d::P_ENV_SENS => signed_db(d::knob_to_env_sens_db(v)),
        d::P_ENV_ATK => seconds(d::knob_to_env_atk_sec(v)),
        d::P_ENV_REL => seconds(d::knob_to_env_rel_sec(v)),
        d::P_LFO_RATE => format!("{:.2} Hz", d::knob_to_lfo_hz(v)),
        d::P_BPM => format!("{:.1}", d::knob_to_bpm(v)),
        d::P_NOTE => {
            let i = (v.round() as i32).clamp(0, 17);
            let ms = 60000.0 / bpm.max(1.0) * d::note_to_beats(i);
            let time = if ms >= 1000.0 { format!("{:.2} s", ms / 1000.0) } else { format!("{} ms", ms.round() as i64) };
            format!("{}  \u{b7}  {time}", NOTES[i as usize])
        }
        d::P_SEQ_STEPS => format!("{} steps", STEP_COUNTS[(v.round() as i32).clamp(0, 14) as usize]),
        d::P_LFO_STEREO => {
            if v <= 0.005 {
                "Mono".to_string()
            } else {
                format!("{}\u{b0}", (v * 180.0).round() as i64)
            }
        }
        d::P_SEQ_GLIDE => {
            if v <= 0.005 {
                "Off".to_string()
            } else {
                format!("{} %", (v * 100.0).round() as i64)
            }
        }
        _ => format!("{}", (v * 10.0).round() as i64),
    }
}

/// Cutoff has no meaningful number of its own -- it is a pedal position -- so the readout is the
/// frequency the DSP is actually running at, which it reports.
pub fn format_cutoff_hz(hz: f32) -> String {
    if hz >= 10000.0 {
        return format!("{:.1} kHz", hz / 1000.0);
    }
    if hz >= 1000.0 {
        return format!("{:.2} kHz", hz / 1000.0);
    }
    format!("{} Hz", hz.round() as i64)
}

// ---------------------------------------------------------------- what applies right now
/// Controls that do not apply to the current mod source are hidden, not greyed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Applicable {
    /// Env Sens / Attack / Release / Detect.
    pub envelope: bool,
    /// Shape / Sync / Rate / Stereo.
    pub lfo: bool,
    /// Steps / Glide / the step row.
    pub steps: bool,
    /// The note value: LFO when synced, and always for Steps.
    pub note: bool,
    /// LFO Rate, only when the LFO is not synced.
    pub free_rate: bool,
}

pub fn what_applies(mod_source: i32, lfo_synced: bool) -> Applicable {
    Applicable {
        envelope: mod_source == 1,
        lfo: mod_source == 2,
        steps: mod_source == 3,
        note: (mod_source == 2 && lfo_synced) || mod_source == 3,
        free_rate: mod_source == 2 && !lfo_synced,
    }
}

/// Whether the editor shows a parameter (`refreshAll`), for the current circuit, mod source, LFO
/// sync and step count: the circuit's own switch rows only, the modulation controls the source
/// uses (Depth whenever something modulates; the step row up to the step count), and the main and
/// setup knobs always. The engine's own Bypass is the header pill.
///
/// Departure for the DAW (controller ruling, as for Mod): the BPM knob and the tempo source are
/// hidden. The session always gives a tempo, which the engine's Host path follows (faithfully), so
/// the knob would do nothing; the parameters stay in the table (and the engine keeps the C++ path).
pub fn shown_for(circuit_index: i32, mod_source: i32, lfo_synced: bool, step_count: i32, index: usize) -> bool {
    let a = what_applies(mod_source, lfo_synced);
    match index {
        d::P_CUTOFF | d::P_RESO | d::P_DRIVE | d::P_MIX | d::P_LEVEL | d::P_CIRCUIT | d::P_TRIM | d::P_MOD_SRC => true,
        d::P_BYPASS => true,
        d::P_TEMPO_SRC | d::P_BPM => false,
        d::P_DEPTH => mod_source != 0,
        d::P_ENV_SENS | d::P_ENV_ATK | d::P_ENV_REL | d::P_ENV_SRC => a.envelope,
        d::P_LFO_SHAPE | d::P_LFO_SYNC | d::P_LFO_STEREO => a.lfo,
        d::P_LFO_RATE => a.free_rate,
        d::P_NOTE => a.note,
        d::P_SEQ_STEPS | d::P_SEQ_GLIDE => a.steps,
        i if (d::P_STEP1..=d::P_STEP16).contains(&i) => a.steps && ((i - d::P_STEP1) as i32) < step_count,
        i => circuit(circuit_index).switches.iter().any(|s| s.param == i),
    }
}

/// What a parameter is called on the current circuit: Cutoff and Reso take the circuit's dial
/// names, a switch its row label; `None` keeps the parameter's own name.
pub fn knob_label(circuit_index: i32, index: usize) -> Option<&'static str> {
    let c = circuit(circuit_index);
    match index {
        d::P_CUTOFF => Some(c.dial_cutoff),
        d::P_RESO => Some(c.dial_reso),
        i => c.switches.iter().find(|s| s.param == i).map(|s| s.label),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_the_editor() {
        assert_eq!(format_value(d::P_LEVEL, 0.6667, 120.0), "+0.0 dB");
        assert_eq!(format_value(d::P_LEVEL, 1.0, 120.0), "+12.0 dB");
        assert_eq!(format_value(d::P_TRIM, 0.3333, 120.0), "+0.0 dB");
        assert_eq!(format_value(d::P_TRIM, 0.0, 120.0), "-12.0 dB");
        assert_eq!(format_value(d::P_DRIVE, 0.0, 120.0), "Off");
        assert_eq!(format_value(d::P_DRIVE, 0.5, 120.0), "+12.0 dB");
        assert_eq!(format_value(d::P_MIX, 1.0, 120.0), "Wet only");
        assert_eq!(format_value(d::P_MIX, 0.5, 120.0), "50 %");
        assert_eq!(format_value(d::P_DEPTH, 0.5, 120.0), "None");
        assert_eq!(format_value(d::P_DEPTH, 0.85, 120.0), "Up 70 %");
        assert_eq!(format_value(d::P_DEPTH, 0.0, 120.0), "Down 100 %");
        assert_eq!(format_value(d::P_ENV_ATK, 0.0, 120.0), "1 ms");
        assert_eq!(format_value(d::P_ENV_REL, 1.0, 120.0), "2.00 s");
        assert_eq!(format_value(d::P_LFO_RATE, 0.0, 120.0), "0.05 Hz");
        assert_eq!(format_value(d::P_BPM, 0.4, 120.0), "120.0");
        assert_eq!(format_value(d::P_NOTE, 9.0, 120.0), "1/4  \u{b7}  500 ms");
        assert_eq!(format_value(d::P_NOTE, 0.0, 120.0), "4 Bars  \u{b7}  8.00 s");
        assert_eq!(format_value(d::P_SEQ_STEPS, 6.0, 120.0), "8 steps");
        assert_eq!(format_value(d::P_LFO_STEREO, 0.0, 120.0), "Mono");
        assert_eq!(format_value(d::P_LFO_STEREO, 1.0, 120.0), "180\u{b0}");
        assert_eq!(format_value(d::P_SEQ_GLIDE, 0.1, 120.0), "10 %");
        assert_eq!(format_value(d::P_CUTOFF, 0.5, 120.0), "5");
        assert_eq!(format_cutoff_hz(440.4), "440 Hz");
        assert_eq!(format_cutoff_hz(1234.0), "1.23 kHz");
        assert_eq!(format_cutoff_hz(12_345.0), "12.3 kHz");
    }

    #[test]
    fn circuits_show_their_own_switches_and_names() {
        assert_eq!(CIRCUITS.len(), 9);
        assert!(shown_for(0, 1, true, 8, d::P_QT_MODE));
        assert!(!shown_for(0, 1, true, 8, d::P_MU_MODE));
        assert!(shown_for(2, 1, true, 8, d::P_CB_VOICE));
        assert!(!shown_for(3, 1, true, 8, d::P_CB_VOICE));
        assert!(shown_for(8, 1, true, 8, d::P_WASP_MODE));
        assert_eq!(knob_label(2, d::P_CUTOFF), Some("Pedal"));
        assert_eq!(knob_label(4, d::P_RESO), Some("Emphasis"));
        assert_eq!(knob_label(2, d::P_CB_VOICE), Some("Sweep"));
        assert_eq!(knob_label(0, d::P_DRIVE), None);
        // the mod source decides the rest
        assert!(!shown_for(0, 0, true, 8, d::P_DEPTH));
        assert!(shown_for(0, 1, true, 8, d::P_ENV_SRC));
        assert!(!shown_for(0, 2, true, 8, d::P_ENV_SENS));
        assert!(shown_for(0, 2, true, 8, d::P_NOTE));
        assert!(!shown_for(0, 2, true, 8, d::P_LFO_RATE));
        assert!(shown_for(0, 2, false, 8, d::P_LFO_RATE));
        assert!(shown_for(0, 3, true, 8, d::P_STEP1 + 7));
        assert!(!shown_for(0, 3, true, 8, d::P_STEP1 + 8));
        assert!(!shown_for(0, 2, true, 16, d::P_STEP1));
        // the session tempo drives sync in the DAW
        assert!(!shown_for(0, 2, true, 8, d::P_BPM));
        assert!(!shown_for(0, 2, true, 8, d::P_TEMPO_SRC));
    }
}
