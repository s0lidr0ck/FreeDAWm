//! Scout's starting prompt: what FreeDAWm and Scout are, how a Scout package reads and how to
//! answer it (the `scout-guides` reply format), and every module the plugin lists offer
//! (`soundcraft_dsp::listed_plugins`: the FireStation engines, the utilities and the instruments)
//! with every parameter's id, label, range, unit and default. Generated from the plugin registry
//! each time, so it never goes stale; the one hand-written part is each module's one-line purpose
//! ([`purpose`], which a test keeps complete).
//!
//! Compact by design (one line per parameter; a parameter repeated per band or slot is one line
//! for all of them): a test keeps it under [`MAX_CHARS`].

use serde_json::{Value, json};
use soundcraft_dsp::{Category, ParamInfo, PluginInfo, Unit};
use std::fmt::Write as _;

/// The prompt stays under this many characters (about 11 k tokens). Raised from 40 000 for Juke (its
/// 33 pad parameters, one line each for all 16 pads, add 2 800) and Lift - Sampler (its filter, LFO,
/// slice and warp parameters; its 64 slices are one line), and from 45 000 for CopyQ (its 24
/// parameters add 1 000).
pub const MAX_CHARS: usize = 46_000;

/// The reply format Scout reads guides from: the `guides` block of a package's
/// `ai_response_instructions`, and the starting prompt's "How to reply".
pub fn guides_format() -> Value {
    json!({
        "when": "Whenever you suggest a change, end the reply with ONE ```json block in exactly this format",
        "example": {
            "format": "scout-guides",
            "version": 1,
            "guides": [
                {"kind": "eq", "track": "<track name>", "lo_hz": 750.0, "hi_hz": 850.0, "gain_lo_db": -4.0, "gain_hi_db": -2.0, "dynamic": true, "q": 2.0, "label": "held-note honk"},
                {"kind": "module", "track": "<track name>", "module": "Comp", "param": "Threshold", "current": "-18.0 dB", "range": "-22 to -24 dB", "reason": "the vocal sits 1.5 LU low against the drum bus"},
            ],
        },
        "rules": [
            "track must be a track.name from this package",
            "lo_hz < hi_hz, 20..20000",
            "gain range -30..30 dB",
            "numbers must come from the measurements in this package",
        ],
    })
}

/// One line on what a listed module is for. Every listed plugin has one (a test checks); an
/// unknown id gets an empty string.
pub fn purpose(id: &str) -> &'static str {
    match id {
        "fs_amp" => "Guitar/bass amp: seven tube amps solved from their schematics, built-in or impulse-response cabinet, Auto Level, output trim.",
        "fs_chamber" => "Reverb chamber you build yourself: room geometry, materials, speaker and mic placement.",
        "fs_cleanq" => {
            "24-band dynamic EQ (bells, shelves, cuts; any band can be dynamic) with a dynamic ceiling; Scout's EQ guides are drawn on it."
        }
        "fs_comp" => "Compressor: ten compressor styles in three lanes (parallel, multiband or serial) plus a dry fader.",
        "fs_delay" => {
            "Delay: six voicings (Digital, Rack, BBD, Tape, Drum, Oil Can), four routings, synced note values, freeze, reverse, pitch in the loop, multi-head taps."
        }
        "fs_dist" => "Distortion/overdrive/fuzz: twelve component-level pedal circuits.",
        "fs_filter" => {
            "Envelope/auto filter: Q-Tron, Mu-Tron, Cry Baby, Vox, Moog, MS-20, SEM, TB-303, Wasp with manual, envelope, LFO or step modulation."
        }
        "fs_levelup" => {
            "LUFS gain-matching leveler for honest A/B (IN/OUT pairing), reference tracks, BS.1770 delivery metering, platform preview, dither."
        }
        "fs_mod" => "Modulation: fourteen circuits (BBD choruses and flangers, phasers, tremolos, vibratos).",
        "fs_pitch" => "Pitch effects: octaver/harmonizer/whammy (POG, Micro POG, OC-2, H910, Whammy).",
        "fs_reverb" => "Reverb: Room and Hall, EMT 140 plate, spring tank, Cloud, Shimmer; decay up to 30 s, synced pre-delay, freeze, era colours.",
        "fs_sat" => "Saturation: tube/transformer/tape-style harmonic colour with drive and mix.",
        "fs_shale" => "Drum gate: band-limited onset detection, lookahead gate following each hit's decay, DeBleed, Kit Link, mic phase alignment.",
        "fs_swale" => "Automatic volume swells (Pick, One-Shot, Rhythmic) with Clean or Slow Gear VCA, Bloom, Alternate.",
        "fs_tape" => "Tape machine: saturation, compression, wow/flutter and head-bump colour.",
        "fs_manners" => {
            "Automatic balance EQ: many times a second it turns down what covers its neighbours (Tame) and lifts what is covered (Recover); for buses and masters, in small amounts."
        }
        "fs_copyq" => {
            "Match EQ: makes a sound's tone meet a measured profile (`mix.copyq_measure` measures one, `mix.module_copyq` sets what a module matches to); link groups make tracks match one another."
        }
        "fs_transient" => "Transient shaper: attack and sustain on anything.",
        "fs_twine" => "Mastering clipper and true-peak limiter, married; the last module on a master.",
        "fs_vint" => "Vintage EQ / colour boxes: seven circuits (four passive/active EQs solved by nodal analysis) plus the MEQ-5.",
        "drum_synth" => "Instrument: synthesized drum kit (kick, snare, hats, toms) played from MIDI.",
        "sampler" => "Instrument: plays an audio sample across the keyboard from MIDI.",
        "juke" => {
            "Instrument: Juke drum machine, 16 pads of samples or synthesized drums (General MIDI notes), with choke groups, sends to two effect buses and step patterns as clips."
        }
        "subtractive_synth" => "Instrument: subtractive synthesizer (oscillators, filter, envelopes).",
        "time_shift" => "Utility: shifts the signal in time (sample-accurate delay or advance) to align tracks.",
        "gain" => "Utility: clean gain change.",
        "trim" => "Utility: input trim for gain staging.",
        "invert" => "Utility: polarity (phase) inversion per channel.",
        "dc_offset_removal" => "Utility: removes DC offset (a very low high-pass).",
        "signal_generator" => "Utility: test tones and noise (replaces the input).",
        _ => "",
    }
}

fn category_name(c: Category) -> &'static str {
    match c {
        Category::FireStation => "FireStation modules",
        Category::Instrument => "Instruments",
        _ => "Utilities",
    }
}

fn unit_name(u: Unit) -> &'static str {
    match u {
        Unit::None => "",
        Unit::Db => "dB",
        Unit::Hz => "Hz",
        Unit::Ms => "ms",
        Unit::Percent => "%",
        Unit::Ratio => ":1",
        Unit::Semitones => "st",
        Unit::Cents => "ct",
        Unit::Seconds => "s",
        Unit::Toggle => "on/off",
        Unit::Choice => "choice",
    }
}

/// A number without trailing zeros (`2`, `0.5`, `-24.25`).
fn n(v: f32) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let s = format!("{:.3}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

/// Digit runs replaced by `#`, and the first digit run (the slot number).
fn shape(s: &str) -> (String, Option<String>) {
    let mut out = String::with_capacity(s.len());
    let mut first: Option<String> = None;
    let mut run = String::new();
    let flush = |out: &mut String, run: &mut String, first: &mut Option<String>| {
        if !run.is_empty() {
            out.push('#');
            if first.is_none() {
                *first = Some(std::mem::take(run));
            }
            run.clear();
        }
    };
    for c in s.chars() {
        if c.is_ascii_digit() {
            run.push(c);
        } else {
            flush(&mut out, &mut run, &mut first);
            out.push(c);
        }
    }
    flush(&mut out, &mut run, &mut first);
    (out, first)
}

/// `0=A, 1=B, …`; a run of numbered labels (`Band 1` … `Band 24`) is one entry `1..24=Band 1..Band 24`.
fn choice_list(choices: &[&str]) -> String {
    let num = |c: &str| shape(c).1.and_then(|d| d.parse::<u64>().ok());
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < choices.len() {
        let c = choices.get(i).copied().unwrap_or("");
        let (k, _) = shape(c);
        let mut j = i;
        while let (Some(a), Some(b)) = (choices.get(j), choices.get(j + 1)) {
            let next = num(a).and_then(|x| x.checked_add(1));
            if k.contains('#') && shape(b).0 == k && next.is_some() && num(b) == next {
                j += 1;
            } else {
                break;
            }
        }
        if j >= i + 2 {
            parts.push(format!("{i}..{j}={c}..{}", choices.get(j).copied().unwrap_or("")));
        } else {
            for (x, c) in choices.iter().enumerate().take(j + 1).skip(i) {
                parts.push(format!("{x}={c}"));
            }
        }
        i = j + 1;
    }
    parts.join(", ")
}

/// The value part of a parameter line: range and default, or the choices.
fn value_spec(p: &ParamInfo) -> String {
    match p.unit {
        Unit::Toggle => format!("on/off ({})", if p.default >= 0.5 { "on" } else { "off" }),
        Unit::Choice => {
            let def = p.choices.get(p.default.max(0.0).round() as usize).copied().unwrap_or("");
            if p.choices.is_empty() {
                format!("{}..{} ({})", n(p.min), n(p.max), n(p.default))
            } else {
                format!("{} ({def})", choice_list(p.choices))
            }
        }
        // A normalized control with its own display: the shown values too.
        _ if p.display.is_some() && p.min == 0.0 && p.max == 1.0 => format!("~{}..{} ({})", p.format(p.min), p.format(p.max), p.format(p.default)),
        _ if p.display.is_some() => format!("{}..{} = {}..{} ({})", n(p.min), n(p.max), p.format(p.min), p.format(p.max), p.format(p.default)),
        u => {
            let unit = unit_name(u);
            let sep = if unit.is_empty() || unit == ":1" || unit == "%" { "" } else { " " };
            format!("{}..{}{sep}{unit} ({})", n(p.min), n(p.max), n(p.default))
        }
    }
}

/// One module's section: heading, purpose, then its parameters (a parameter repeated per band or
/// slot, same range and unit, is one line naming the slots).
fn module_section(out: &mut String, info: &PluginInfo) {
    let _ = writeln!(out, "### {} (`{}`, slot name \"{}\")", info.name, info.id, info.short_name);
    let what = purpose(info.id);
    if !what.is_empty() {
        let _ = writeln!(out, "{what}");
    }
    if info.sidechain {
        let _ = writeln!(out, "Takes a sidechain key from another track.");
    }
    // Group the parameters by shape (id and label with digits as `#`, range, unit, choices).
    type Key = (String, String, u32, u32, &'static str, usize);
    let key = |p: &ParamInfo| -> Key {
        let (id, _) = shape(p.id);
        let (name, _) = shape(p.name);
        (id, name, p.min.to_bits(), p.max.to_bits(), unit_name(p.unit), p.choices.len())
    };
    let mut groups: Vec<(Key, Vec<&ParamInfo>)> = Vec::new();
    for p in info.params {
        let k = key(p);
        match groups.iter_mut().find(|(g, _)| *g == k) {
            Some((_, v)) if k.0.contains('#') => v.push(p),
            _ => groups.push((k, vec![p])),
        }
    }
    for (k, ps) in &groups {
        match ps.as_slice() {
            [p] => {
                // A choice list already given in this module is named, not repeated.
                let same =
                    info.params.iter().take_while(|q| q.id != p.id).find(|q| q.unit == Unit::Choice && q.choices.len() > 3 && q.choices == p.choices);
                let spec = match same {
                    Some(q) if p.unit == Unit::Choice => {
                        format!("choices as `{}` ({})", q.id, p.choices.get(p.default.max(0.0).round() as usize).copied().unwrap_or(""))
                    }
                    _ => value_spec(p),
                };
                let _ = writeln!(out, "- `{}` {}: {spec}", p.id, p.name);
            }
            many => {
                let slots: Vec<String> = many.iter().filter_map(|p| shape(p.id).1).collect();
                let range = match (slots.first(), slots.last()) {
                    (Some(a), Some(b)) => format!("{a}–{b}"),
                    _ => String::new(),
                };
                let first = many.first().copied();
                let same_default = many.iter().all(|p| first.is_some_and(|f| f.default.to_bits() == p.default.to_bits()));
                let spec = first.map(value_spec).unwrap_or_default();
                let spec = if same_default {
                    spec
                } else {
                    let defaults: Vec<String> = many.iter().map(|p| n(p.default)).collect();
                    let head = spec.rsplit_once(" (").map_or(spec.as_str(), |(h, _)| h);
                    format!("{head} (defaults {})", defaults.join(", "))
                };
                let _ = writeln!(out, "- `{}` ×{} (# = {range}) {}: {spec}", k.0, many.len(), k.1);
            }
        }
    }
    out.push('\n');
}

/// The whole starting prompt (Markdown).
pub fn starting_prompt() -> String {
    let mut out = String::with_capacity(32 * 1024);
    out.push_str("# FreeDAWm + Scout: starting prompt\n\n");
    out.push_str(
        "You are helping an audio engineer mix and master in FreeDAWm, a digital audio workstation whose signal chains are built from the FireStation modules listed below. \
Use only these modules and their parameters when you suggest processing; name a module by its name and a parameter by its label, and keep every value inside the listed range.\n\n",
    );
    out.push_str("## Scout\n\n");
    out.push_str(
        "Scout measures the mix while it plays. A capture records each chosen strip (track, bus or master) PRE (before its first module) and POST (after its last module). \
The engineer then sends you a Scout package: JSON with, per strip, the measurements (level, true peak, LUFS and loudness range, crest factor and dynamics, \
a 31-band 1/3-octave spectrum with its tilt and resonances, broad zones, stereo width and correlation, transient density), the chain of modules with their settings as the engineer sees them, \
the routing, flags Scout raised, the engineer's question and session goal, and its own interpretation guide and response instructions. \
A strip may carry a target: a sound profile (the same measurements taken from a reference sound, a stem of a reference song or a finished mix) with the differences per band and metric; \
move the strip toward its target. A target on the master is a mastering target.\n\n",
    );
    out.push_str("## How to reply\n\n");
    out.push_str(
        "Answer in sections (summary, observations, suggestions, warnings). Reference the measured numbers, give parameter ranges the engineer sets by hand, never tell them to load a preset. \
Scout draws EQ guides on the track's Clean Q and lists module suggestions; it reads them from the reply:\n\n",
    );
    let g = guides_format();
    let _ = writeln!(out, "{}", g["when"].as_str().unwrap_or(""));
    let _ = writeln!(out, "\n```json\n{}\n```\n", serde_json::to_string_pretty(&g["example"]).unwrap_or_default());
    out.push_str("Rules:\n");
    for r in g["rules"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let _ = writeln!(out, "- {r}");
    }
    out.push_str(
        "\n## The modules\n\nParameter lines read: `id` Label: min..max unit (default). `~x..y` is a 0..1 control showing x..y; `a..b = x..y` is raw range a..b showing x..y. A line with `#` stands for one parameter per band or slot (# = the slot numbers shown). \
Choices list index=label (default label).\n\n",
    );
    let mut cats: Vec<Category> = Vec::new();
    for p in soundcraft_dsp::listed_plugins() {
        let c = match p.category {
            Category::FireStation | Category::Instrument => p.category,
            _ => Category::Other,
        };
        if !cats.contains(&c) {
            cats.push(c);
        }
    }
    // FireStation first, then the utilities, then the instruments.
    cats.sort_by_key(|c| match c {
        Category::FireStation => 0,
        Category::Instrument => 2,
        _ => 1,
    });
    for c in cats {
        let _ = writeln!(out, "## {}\n", category_name(c));
        for p in soundcraft_dsp::listed_plugins() {
            let pc = match p.category {
                Category::FireStation | Category::Instrument => p.category,
                _ => Category::Other,
            };
            if pc == c {
                module_section(&mut out, p);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_module_and_parameter_is_in_the_prompt() {
        let text = starting_prompt();
        for p in soundcraft_dsp::listed_plugins() {
            assert!(text.contains(&format!("(`{}`", p.id)), "{} missing", p.id);
            assert!(!purpose(p.id).is_empty(), "{} needs a purpose line", p.id);
            for q in p.params {
                let (shaped, _) = shape(q.id);
                assert!(text.contains(&format!("`{}`", q.id)) || text.contains(&format!("`{shaped}`")), "{}.{} missing", p.id, q.id);
            }
        }
        assert!(text.contains("scout-guides"));
    }

    #[test]
    fn the_prompt_stays_compact() {
        let text = starting_prompt();
        if let Some(out) = std::env::var_os("SCOUT_PROMPT_OUT") {
            let _ = std::fs::write(out, &text);
        }
        assert!(text.len() < MAX_CHARS, "starting prompt is {} chars", text.len());
    }

    #[test]
    fn repeated_band_parameters_collapse_to_one_line() {
        let text = starting_prompt();
        assert!(text.contains("`b#_gain` ×24 (# = 01–24)"), "{}", text.lines().filter(|l| l.contains("b#_")).collect::<Vec<_>>().join("\n"));
    }

    #[test]
    fn numbers_print_short() {
        assert_eq!(n(2.0), "2");
        assert_eq!(n(-0.0), "0");
        assert_eq!(n(0.25), "0.25");
        assert_eq!(n(f32::NAN), "0");
        assert_eq!(choice_list(&["None", "Band 1", "Band 2", "Band 3", "Off"]), "0=None, 1..3=Band 1..Band 3, 4=Off");
        assert_eq!(choice_list(&["2x", "4x"]), "0=2x, 1=4x");
        assert_eq!(shape("b12_gain"), ("b#_gain".to_string(), Some("12".to_string())));
    }
}
