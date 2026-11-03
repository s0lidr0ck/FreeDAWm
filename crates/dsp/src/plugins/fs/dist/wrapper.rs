//! DistEngine's wrapper logic (`PluginProcessor.cpp`, `PedalConfig.h`) that changes the sound by
//! moving parameters: Link (Level follows Drive between a circuit's two Level set points), the
//! per-circuit knob memory (flipping circuits recalls that circuit's own Drive, Tone and Level),
//! and the matched Level a preset load gets. In the C++ these run on the message thread
//! (`AsyncUpdater`); here they are the plugin's derived parameters ([`crate::derive`]), run by the
//! host's command layer after each edit, so they are undoable and an automation lane wins.

use crate::derive::{Derive, ParamEdit};

/// One circuit's entry of `PedalConfig.h`'s table: its knob names, its Link anchors (the Drive
/// positions the Link curve is pinned at) and the params holding its Level set points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CircuitDef {
    pub name: &'static str,
    /// What the Drive, Tone and Level dials are called on this pedal.
    pub dial_drive: &'static str,
    pub dial_tone: &'static str,
    pub dial_level: &'static str,
    pub anchor_lo: f32,
    pub anchor_hi: f32,
    pub link_lo: &'static str,
    pub link_hi: &'static str,
    /// The circuit's switch rows: (param id, row label), as the editor shows them.
    pub switches: &'static [(&'static str, &'static str)],
}

const fn c(
    name: &'static str,
    dials: [&'static str; 3],
    anchors: (f32, f32),
    link: (&'static str, &'static str),
    switches: &'static [(&'static str, &'static str)],
) -> CircuitDef {
    CircuitDef {
        name,
        dial_drive: dials[0],
        dial_tone: dials[1],
        dial_level: dials[2],
        anchor_lo: anchors.0,
        anchor_hi: anchors.1,
        link_lo: link.0,
        link_hi: link.1,
        switches,
    }
}

/// `getCircuits()`, in the order of the Circuit parameter.
pub static CIRCUITS: [CircuitDef; 12] = [
    c("Tube Screamer", ["Drive", "Tone", "Level"], (0.0, 1.0), ("tsLo", "tsHi"), &[("tsModel", "Model"), ("tsDiode", "Diode")]),
    c(
        "RAT",
        ["Distortion", "Filter", "Volume"],
        (0.1, 1.0),
        ("ratLo", "ratHi"),
        &[("ratModel", "Model"), ("ratDiode", "Diode"), ("ratChip", "Chip"), ("ratFat", "Fat")],
    ),
    c(
        "Big Muff",
        ["Sustain", "Tone", "Volume"],
        (0.15, 1.0),
        ("muffLo", "muffHi"),
        &[("muffVer", "Version"), ("muffDiode", "Diode"), ("muffMids", "Mids")],
    ),
    c("Fuzz Face", ["Fuzz", "Cap", "Volume"], (0.2, 1.0), ("ffLo", "ffHi"), &[("ffTrans", "Transistor"), ("ffBias", "Bias")]),
    c("DS-1", ["Dist", "Tone", "Level"], (0.1, 1.0), ("ds1Lo", "ds1Hi"), &[("ds1Diode", "Diode"), ("ds1Voice", "Voice")]),
    c("Klon", ["Gain", "Treble", "Output"], (0.0, 1.0), ("klonLo", "klonHi"), &[("klonDiode", "Diode")]),
    c("Blues Breaker", ["Gain", "Tone", "Volume"], (0.0, 1.0), ("bbLo", "bbHi"), &[("bbGain", "Gain")]),
    c("Guv'nor", ["Gain", "Treble", "Level"], (0.05, 1.0), ("govLo", "govHi"), &[("govClip", "Clip"), ("govBass", "Bass"), ("govMid", "Middle")]),
    c("Tube Driver", ["Drive", "Hi", "Level"], (0.0, 1.0), ("tdLo", "tdHi"), &[("tdPlate", "Plate"), ("tdBias", "Bias")]),
    c("Blues Driver", ["Gain", "Tone", "Level"], (0.0, 1.0), ("bdLo", "bdHi"), &[("bdMode", "Mode")]),
    c("Dist+", ["Distortion", "Cap", "Output"], (0.0, 1.0), ("dpLo", "dpHi"), &[("dpDiode", "Diode")]),
    c("OCD", ["Drive", "Tone", "Volume"], (0.0, 1.0), ("ocdLo", "ocdHi"), &[("ocdMode", "HP / LP")]),
];

/// `getCircuitStartPoints()`: (Drive, Tone) where each circuit's knobs start before it has been
/// touched. Level is not listed: it is derived from the circuit's Link anchors at that Drive.
pub static START_POINTS: [(f32, f32); 12] = [
    (0.35, 0.60), // TS is a mid boost, not a distortion: low Drive, Tone a little up.
    (0.45, 0.25), // RAT Filter is REVERSED -- up is darker -- so 0.25 is the bright, classic setting.
    (0.75, 0.50), // A Big Muff lives near the top of Sustain. Tone at the scoop.
    (0.85, 0.60), // Fuzz near max and clean it up with Input; Cap up is brighter.
    (0.45, 0.45), // DS-1 at halfway, Tone just under noon to keep the fizz down.
    (0.30, 0.50), // Klon is a transparent low-gain boost; Treble shelf flat at noon.
    (0.40, 0.60), // Blues Breaker is low-gain and transparent; passive Tone fairly open.
    (0.50, 0.50), // Guv'nor is the amp-in-a-box one: everything at noon is its sound.
    (0.50, 0.50), // Tube Driver at noon; the Plate and Bias switches do the character.
    (0.40, 0.50), // BD-2 Tone is a tilt around 1 kHz, so noon is genuinely flat.
    (0.55, 0.25), // Dist+ has no tone control: Cap low keeps it near the stock 1 nF.
    (0.40, 0.50), // OCD at a useful edge-of-breakup; HP/LP does the rest.
];

/// `juce::jlimit (lower, upper, v)`: `v < lower ? lower : (upper < v ? upper : v)`.
fn jlimit<T: PartialOrd>(lower: T, upper: T, v: T) -> T {
    if v < lower {
        lower
    } else if upper < v {
        upper
    } else {
        v
    }
}

/// The circuit table entry for a (rounded, clamped) circuit index.
pub fn circuit(index: i32) -> &'static CircuitDef {
    let i = jlimit(0, CIRCUITS.len() as i32 - 1, index) as usize;
    &CIRCUITS[i.min(CIRCUITS.len() - 1)]
}

/// `linkWeight`: the pedal's saturating curve, 0 at the low anchor and 1 at the high one.
pub fn link_weight(drive: f32, anchor_lo: f32, anchor_hi: f32) -> f32 {
    let span = if (anchor_hi - anchor_lo) != 0.0 { anchor_hi - anchor_lo } else { 1.0 };
    let t = jlimit(0.0f32, 1.0, (drive - anchor_lo) / span);
    1.0 - (1.0 - t).powf(2.2)
}

/// `(int) std::round (getParamValue (pCircuit))`.
fn circuit_index(v: f32) -> i32 {
    v.round() as i32
}

/// `linkLevelFor`: the Level the Link curve would choose for `circuit` at `drive`, given the
/// parameter values (`get`). Both set points at (nearly) zero means an uncalibrated circuit:
/// noon, rather than silence.
pub fn link_level_for(get: impl Fn(&str) -> f32, circuit_idx: i32, drive: f32) -> f32 {
    let c = circuit(circuit_idx);
    let lo = get(c.link_lo);
    let hi = get(c.link_hi);
    if lo < 0.001 && hi < 0.001 {
        return 0.5;
    }
    lo + (hi - lo) * link_weight(drive, c.anchor_lo, c.anchor_hi)
}

/// `applyLink`'s decision: the Level Link sets, `None` when it leaves Level alone (Link off, an
/// uncalibrated circuit, or already within 0.0005 of the target). `get` reads parameter values.
/// Shared by the command layer ([`derive`]) and the plugin itself (automation).
pub fn link_target(get: impl Fn(&str) -> f32) -> Option<f32> {
    if get("link") < 0.5 {
        return None;
    }
    // (calibrationActive: the editor's calibration wizard is not ported, so it is never active.)
    let c = circuit(circuit_index(get("circuit")));
    let lo_level = get(c.link_lo);
    let hi_level = get(c.link_hi);
    // Both set points at zero: no usable calibration. Leave Level where it is rather than mute.
    if lo_level < 0.001 && hi_level < 0.001 {
        return None;
    }
    let drive = get("drive");
    let target = lo_level + (hi_level - lo_level) * link_weight(drive, c.anchor_lo, c.anchor_hi);
    if (target - get("level")).abs() < 0.0005 {
        return None;
    }
    Some(target)
}

/// `applyLink`: with Link on, Level follows Drive between the circuit's two set points.
fn apply_link(d: &mut Derive<'_>) {
    if let Some(target) = link_target(|id| d.get(id)) {
        d.set("level", target);
    }
}

fn slot_key(circuit: i32, knob: &str) -> String {
    format!("{circuit}.{knob}")
}

/// `rememberKnobs`: stash the live Drive, Tone and Level in `circuit`'s slot.
fn remember_knobs(d: &mut Derive<'_>, circuit: i32) {
    for knob in ["drive", "tone", "level"] {
        let v = d.get(knob);
        d.memory.insert(slot_key(circuit, knob), v);
    }
}

/// `recallKnobs`: bring back `circuit`'s Drive and Tone (seeding the slot from the start point
/// the first time), and its Level only with Link off (Link owns Level).
fn recall_knobs(d: &mut Derive<'_>, circuit: i32) {
    if !d.memory.contains_key(&slot_key(circuit, "drive")) {
        // seedKnobSlot
        let i = jlimit(0, START_POINTS.len() as i32 - 1, circuit) as usize;
        let (drive, tone) = START_POINTS.get(i).copied().unwrap_or((0.5, 0.5));
        let level = link_level_for(|id| d.get(id), circuit, drive);
        d.memory.insert(slot_key(circuit, "drive"), drive);
        d.memory.insert(slot_key(circuit, "tone"), tone);
        d.memory.insert(slot_key(circuit, "level"), level);
    }
    // A missing property reads as 0, as a `juce::var` does.
    let slot = |d: &Derive<'_>, knob: &str| d.memory.get(&slot_key(circuit, knob)).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
    let (drive, tone, level) = (slot(d, "drive"), slot(d, "tone"), slot(d, "level"));
    d.set("drive", drive);
    d.set("tone", tone);
    if d.get("link") < 0.5 {
        d.set("level", level);
    }
}

/// The parameters whose changes the C++ processor listens to (Link's inputs and everything a
/// preset captures): all but Level, Input trim and Bypass. Only these start the async update.
pub fn listened(id: &str) -> bool {
    !matches!(id, "level" | "trim" | "bypass")
}

/// DistEngine's derived parameters, run after each edit:
/// - an edit (a control, a command, automation written by hand): a changed circuit stashes the
///   outgoing circuit's knobs and recalls the incoming one's (`applyPendingKnobSwap`), then Link
///   runs (`handleAsyncUpdate`);
/// - a preset: no knob swap (the editor suppresses it), the matched Level when Link is off and
///   the preset carries no Level (`applyMatchedLevel`: no C++ preset stores one), the bank
///   learns the loaded knobs (`rememberKnobs`), then Link.
pub fn derive(d: &mut Derive<'_>) {
    if d.changed().is_empty() {
        return; // nothing moved (a preset rename): no listener fires
    }
    let async_update = d.changed().iter().any(|id| listened(id));
    match d.edit() {
        ParamEdit::Edit => {
            let (was, now) = (circuit_index(d.before("circuit")), circuit_index(d.get("circuit")));
            if d.was_set("circuit") && was != now {
                remember_knobs(d, was);
                recall_knobs(d, now);
            }
        }
        ParamEdit::Preset => {
            let now = circuit_index(d.get("circuit"));
            if d.get("link") < 0.5 && !d.was_set("level") {
                let level = link_level_for(|id| d.get(id), now, d.get("drive"));
                d.set("level", level);
            }
            remember_knobs(d, now);
        }
    }
    if async_update {
        apply_link(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct Run {
        values: BTreeMap<String, f32>,
        memory: BTreeMap<String, f32>,
    }

    impl Run {
        fn new() -> Self {
            Run { values: BTreeMap::new(), memory: BTreeMap::new() }
        }
        fn edit(&mut self, kind: ParamEdit, set: &[(&str, f32)], locked: &dyn Fn(&str) -> bool) {
            let before = self.values.clone();
            let info = crate::plugin_info("fs_dist").unwrap();
            for (k, v) in set {
                self.values.insert((*k).to_string(), info.param(k).unwrap().clamp(*v));
            }
            let changed: Vec<String> = set.iter().map(|(k, _)| (*k).to_string()).collect();
            let mut d = Derive::new(info, &before, &mut self.values, &mut self.memory, &changed, kind, locked);
            derive(&mut d);
        }
        fn set(&mut self, set: &[(&str, f32)]) {
            self.edit(ParamEdit::Edit, set, &|_| false);
        }
        fn get(&self, id: &str) -> f32 {
            let info = crate::plugin_info("fs_dist").unwrap();
            self.values.get(id).copied().unwrap_or(info.param(id).unwrap().default)
        }
    }

    #[test]
    fn link_weight_matches_the_cpp_curve() {
        assert_eq!(link_weight(0.0, 0.0, 1.0), 0.0);
        assert_eq!(link_weight(1.0, 0.0, 1.0), 1.0);
        assert!((link_weight(0.5, 0.0, 1.0) - (1.0 - 0.5f32.powf(2.2))).abs() < 1e-7);
        assert_eq!(link_weight(0.05, 0.1, 1.0), 0.0, "below the low anchor");
        assert_eq!(link_weight(0.3, 0.5, 0.5), 0.0, "zero span divides by 1");
        assert!(link_weight(f32::NAN, 0.0, 1.0).is_nan(), "jlimit passes NaN like the C++");
    }

    #[test]
    fn link_drives_level_from_drive_and_is_a_no_op_when_off() {
        let mut r = Run::new();
        r.set(&[("drive", 0.8)]);
        assert_eq!(r.get("level"), 0.5, "Link is off by default");
        r.set(&[("link", 1.0)]);
        let want = 0.7 + (0.5 - 0.7) * link_weight(0.8, 0.0, 1.0);
        assert_eq!(r.get("level"), want, "turning Link on applies it");
        r.set(&[("drive", 0.2)]);
        assert_eq!(r.get("level"), 0.7 + (0.5 - 0.7) * link_weight(0.2, 0.0, 1.0));
        // Level itself is not listened to: a hand-set Level stays until the next listened change.
        r.set(&[("level", 0.9)]);
        assert_eq!(r.get("level"), 0.9);
        r.set(&[("tone", 0.3)]);
        assert_ne!(r.get("level"), 0.9, "Tone is listened to (a preset param), so Link re-applies");
        // Uncalibrated circuit (both set points ~0): Level left alone.
        r.set(&[("tsLo", 0.0), ("tsHi", 0.0), ("level", 0.42)]);
        assert_eq!(r.get("level"), 0.42);
    }

    #[test]
    fn an_automation_lane_on_level_wins() {
        let mut r = Run::new();
        r.edit(ParamEdit::Edit, &[("link", 1.0), ("drive", 0.9)], &|id| id == "level");
        assert_eq!(r.get("level"), 0.5, "locked by its lane");
    }

    #[test]
    fn circuit_swap_remembers_and_recalls_each_circuits_knobs() {
        let mut r = Run::new();
        r.set(&[("drive", 0.66), ("tone", 0.11), ("level", 0.77)]);
        r.set(&[("circuit", 3.0)]);
        // Fuzz Face seeds from its start point, Level matched from its anchors at that Drive.
        assert_eq!((r.get("drive"), r.get("tone")), (0.85, 0.60));
        assert_eq!(r.get("level"), 0.6 + (0.4 - 0.6) * link_weight(0.85, 0.2, 1.0));
        r.set(&[("drive", 0.5)]);
        r.set(&[("circuit", 0.0)]);
        assert_eq!((r.get("drive"), r.get("tone"), r.get("level")), (0.66, 0.11, 0.77), "TS remembered");
        r.set(&[("circuit", 3.0)]);
        assert_eq!(r.get("drive"), 0.5, "FF remembered");
        // Re-sending the same circuit is not a swap.
        r.set(&[("drive", 0.33), ("circuit", 3.0)]);
        assert_eq!(r.get("drive"), 0.33);
    }

    #[test]
    fn swap_with_link_on_recalls_drive_and_lets_link_set_level() {
        let mut r = Run::new();
        r.set(&[("link", 1.0)]);
        r.set(&[("circuit", 1.0)]);
        assert_eq!(r.get("drive"), 0.45);
        assert_eq!(r.get("level"), 0.7 + (0.35 - 0.7) * link_weight(0.45, 0.1, 1.0));
    }

    #[test]
    fn a_preset_skips_the_swap_and_gets_the_matched_level() {
        let mut r = Run::new();
        r.edit(ParamEdit::Preset, &[("circuit", 2.0), ("drive", 0.9), ("tone", 0.2)], &|_| false);
        assert_eq!((r.get("drive"), r.get("tone")), (0.9, 0.2), "no recall");
        assert_eq!(r.get("level"), 0.6 + (0.4 - 0.6) * link_weight(0.9, 0.15, 1.0));
        // ...and the bank learned it: flipping away and back returns to the preset.
        r.set(&[("circuit", 0.0)]);
        r.set(&[("circuit", 2.0)]);
        assert_eq!((r.get("drive"), r.get("tone")), (0.9, 0.2));
        // A preset that carries its own Level keeps it.
        r.edit(ParamEdit::Preset, &[("circuit", 2.0), ("drive", 0.9), ("level", 0.31)], &|_| false);
        assert_eq!(r.get("level"), 0.31);
        // An empty preset (a rename) changes nothing.
        let before = r.values.clone();
        r.edit(ParamEdit::Preset, &[], &|_| false);
        assert_eq!(r.values, before);
    }

    #[test]
    fn hostile_memory_reads_as_zero_and_stays_in_range() {
        let mut r = Run::new();
        r.memory.insert("5.drive".into(), f32::NAN);
        r.memory.insert("5.tone".into(), 9.0);
        r.set(&[("circuit", 5.0)]);
        assert_eq!(r.get("drive"), 0.0);
        assert_eq!(r.get("tone"), 1.0, "clamped");
        assert_eq!(r.get("level"), 0.0, "missing reads as 0");
        assert!(r.values.values().all(|v| v.is_finite()));
    }

    #[test]
    fn circuit_table_matches_the_parameter_table() {
        let info = crate::plugin_info("fs_dist").unwrap();
        let names = info.param("circuit").unwrap().choices;
        for (i, c) in CIRCUITS.iter().enumerate() {
            assert_eq!(c.name, names[i]);
            assert!(info.param(c.link_lo).is_some() && info.param(c.link_hi).is_some());
            for (id, _) in c.switches {
                assert!(info.param(id).is_some(), "{id}");
            }
        }
        // Every switch param belongs to exactly one circuit.
        let switches: Vec<&str> = CIRCUITS.iter().flat_map(|c| c.switches.iter().map(|s| s.0)).collect();
        assert_eq!(switches.len(), 23);
    }
}
