//! `ModConfig.h`: the PEDAL table from ModEngineUI.js, as data (the editor's per-circuit tables).
//!
//! Link here has ONE pair of set points for the whole plugin rather than a pair per circuit,
//! because the Cmajor declares only linkLo/linkHi.

/// Which dials a circuit actually uses, and its variant names (empty: no Variant). The C++ editor
/// dims a dial that does nothing; SoundCraft's drawer hides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CircuitDef {
    pub name: &'static str,
    pub variants: &'static [&'static str],
    pub regen: bool,
    pub noise: bool,
    pub bias: bool,
    pub tone: bool,
}

const fn c(name: &'static str, variants: &'static [&'static str], regen: bool, noise: bool, bias: bool, tone: bool) -> CircuitDef {
    CircuitDef { name, variants, regen, noise, bias, tone }
}

/// `getCircuits()`, in circuit order.
pub static CIRCUITS: [CircuitDef; 14] = [
    //                                                          regen  noise  bias   tone
    c("Dim D", &["1", "2", "3", "4", "1 + 4", "2 + 3"], false, true, false, false),
    c("CE-1", &["Chorus", "Vibrato"], false, true, false, false),
    c("CE-2", &[], false, true, false, false),
    c("Small Clone", &[], false, true, false, false),
    c("JC-120", &["Chorus", "Vibrato"], false, true, false, false),
    c("Brigade", &[], false, true, false, false),
    c("Mistress", &[], true, true, false, false),
    c("A/DA", &[], true, true, false, false),
    c("Phase 90", &["Script", "Block"], true, false, true, false),
    c("Small Stone", &["Colour off", "Colour on"], true, false, true, false),
    c("Uni-Vibe", &["Chorus", "Vibrato"], false, false, true, false),
    c("Opto Trem", &[], false, false, true, true),
    c("Harmonic", &[], false, false, true, true),
    c("Magnatone", &["Stereo vib", "Full vibrato"], false, false, true, true),
];

/// The circuit's entry, clamped into the table as the editor does (`jlimit`).
pub fn circuit(index: i32) -> &'static CircuitDef {
    &CIRCUITS[index.clamp(0, CIRCUITS.len() as i32 - 1) as usize]
}

/// The circuit switch's short labels (`getSwitches()`, the Circuit row).
pub static CIRCUIT_SHORT: [&str; 14] =
    ["Dim D", "CE-1", "CE-2", "Clone", "JC-120", "Brigade", "Mistress", "A/DA", "Ph 90", "Stone", "Vibe", "Opto", "Harm", "Magna"];

/// What each circuit is (the Circuit row's descriptions).
pub static CIRCUIT_BLURBS: [&str; 14] = [
    "Roland SDD-320. Two MN3007 lines in anti-phase, trapezoid LFO, cross-fed. Variant = the buttons.",
    "Boss CE-1. MN3002, 512 stages, its own preamp and a noisy floor. Variant: Chorus / Vibrato.",
    "Boss CE-2. MN3007, 1024 stages. The standard clean-guitar chorus.",
    "EHX Small Clone. MN3007, wide fixed-width triangle sweep. Fat and slow.",
    "Roland JC-120 amp chorus. MN3002 pair, cross-fed. Variant: Chorus / Vibrato.",
    "Generic bucket-brigade chorus. MN3007 with a darker filter set, wide and a little noisy.",
    "EHX Electric Mistress. SAD1024 flanger, filter-matrix voicing. Regen is live.",
    "A/DA Flanger. SAD1024, the deepest sweep here, regen to the edge.",
    "MXR Phase 90. Four JFET stages; the audio modulates the FETs. Variant: Script / Block.",
    "EHX Small Stone. Four OTA stages, pole linear in bias current. Variant: Colour off / on.",
    "Shin-ei Uni-Vibe. One lamp with thermal lag into four LDRs with different caps. Variant: Chorus / Vibrato.",
    "Fender blackface tremolo. Neon lamp strike + LDR shunt. Depth = Intensity.",
    "Fender brownface harmonic tremolo. Phase-shift network, bands driven counter-phase. Bias moves the crossover.",
    "Magnatone varistor vibrato. Six phase sections, true pitch FM, no delay line. Variant: Stereo vib / Full vibrato.",
];

/// The other switch rows' labels and positions (`getSwitches()`).
pub const STEREO_SWITCH: (&str, &[&str]) = ("Input", &["Mono", "Stereo"]);
pub const SHAPE_SWITCH: (&str, &[&str]) = ("Shape", &["Stock", "Sine", "Tri", "Trapezoid", "Square"]);
pub const SYNC_SWITCH: (&str, &[&str]) = ("Sync", &["Free", "Sync"]);
pub const DIVISION_SWITCH: (&str, &[&str]) = ("Div", &["1 bar", "1/1", "1/2d", "1/2", "1/4d", "1/4", "1/8", "1/16"]);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_follows_the_engine() {
        // The names line up with Parameters.h `kCircuits`, the variant counts with the engine's
        // own variant branches.
        let info = crate::plugin_info("fs_mod").unwrap();
        let names = info.param("circuit").unwrap().choices;
        assert_eq!(CIRCUITS.iter().map(|c| c.name).collect::<Vec<_>>(), names);
        assert_eq!(circuit(0).variants.len(), 6);
        assert!(circuit(2).variants.is_empty());
        assert_eq!(circuit(-4).name, "Dim D");
        assert_eq!(circuit(99).name, "Magnatone");
        // Regen only where the engine has a feedback path: the flangers and the two phasers.
        let regen: Vec<usize> = (0..14).filter(|&i| CIRCUITS[i].regen).collect();
        assert_eq!(regen, [6, 7, 8, 9]);
    }
}
