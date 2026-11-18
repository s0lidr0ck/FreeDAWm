//! A built-in synth's modulation patch (`Insert::modulation`): the slots of its modulation
//! matrix and its four macros' names and targets. The macros' values are ordinary parameters
//! (`macro1`..`macro4`). What the names mean, and the checks, live with the synths
//! (`soundcraft_dsp::plugins::synth_mod`) and the commands that edit this (`mix.mod_*`).

/// Slots a synth's matrix has.
pub const MAX_MOD_SLOTS: usize = 16;
/// Macros per synth.
pub const MAX_MACROS: usize = 4;
/// Destinations one macro drives.
pub const MAX_MACRO_TARGETS: usize = 8;
/// Longest macro name, characters.
pub const MAX_MACRO_NAME: usize = 24;

/// One slot: `source` moves `dest` by `amount` (-1..1) of the destination knob's travel.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModSlot {
    pub source: String,
    pub dest: String,
    #[serde(default)]
    pub amount: f32,
}

/// One macro target: at macro value v (0..1) `dest` moves by `from + (to - from) * v`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MacroTarget {
    pub dest: String,
    #[serde(default)]
    pub from: f32,
    #[serde(default)]
    pub to: f32,
}

/// A macro's name (empty: "Macro n") and targets.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SynthMacro {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<MacroTarget>,
}

impl SynthMacro {
    fn is_empty(&self) -> bool {
        self.name.is_empty() && self.targets.is_empty()
    }
}

/// A synth's modulation patch. Empty (the default, and what every session from before has):
/// the synth plays as it always did.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SynthMod {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<ModSlot>,
    /// Index 0 is Macro 1; fewer than four: the rest unnamed and unassigned.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub macros: Vec<SynthMacro>,
}

/// An amount kept in range (NaN: 0).
fn amount(x: f32) -> f32 {
    if x.is_finite() { x.clamp(-1.0, 1.0) } else { 0.0 }
}

impl SynthMod {
    /// Nothing patched and no macro named or assigned.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty() && self.macros.iter().all(SynthMacro::is_empty)
    }

    /// Macro `i`'s (0-based) name as shown: its own, or "Macro n".
    pub fn macro_name(&self, i: usize) -> String {
        match self.macros.get(i) {
            Some(m) if !m.name.trim().is_empty() => m.name.clone(),
            _ => format!("Macro {}", i + 1),
        }
    }

    /// Kept within the limits (hostile files): at most 16 slots, 4 macros of 8 targets, names
    /// of at most 24 characters, amounts in -1..1, trailing empty macros dropped.
    pub fn sanitize(&mut self) {
        self.slots.truncate(MAX_MOD_SLOTS);
        for s in &mut self.slots {
            s.amount = amount(s.amount);
        }
        self.macros.truncate(MAX_MACROS);
        for m in &mut self.macros {
            m.name = m.name.trim().chars().take(MAX_MACRO_NAME).collect();
            m.targets.truncate(MAX_MACRO_TARGETS);
            for t in &mut m.targets {
                t.from = amount(t.from);
                t.to = amount(t.to);
            }
        }
        while self.macros.last().is_some_and(SynthMacro::is_empty) {
            self.macros.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_patch_is_left_out_and_old_inserts_load_with_none() {
        let ins: crate::Insert = serde_json::from_str(r#"{"plugin":"subtractive_synth"}"#).unwrap();
        assert!(ins.modulation.is_empty());
        let text = serde_json::to_string(&ins).unwrap();
        assert!(!text.contains("modulation"), "{text}");
        let mut m = SynthMod::default();
        m.macros.push(SynthMacro::default());
        m.sanitize();
        assert!(m.macros.is_empty());
    }

    #[test]
    fn a_patch_round_trips_and_hostile_values_are_kept_in_range() {
        let json = r#"{"plugin":"fm_synth","modulation":{"slots":[{"source":"lfo","dest":"pitch","amount":7}],
            "macros":[{"name":"  Bell  ","targets":[{"dest":"op2_level","from":-3,"to":0.5}]}]}}"#;
        let mut ins: crate::Insert = serde_json::from_str(json).unwrap();
        ins.modulation.sanitize();
        assert_eq!(ins.modulation.slots[0].amount, 1.0);
        assert_eq!(ins.modulation.macros[0].targets[0].from, -1.0);
        assert_eq!(ins.modulation.macro_name(0), "Bell");
        assert_eq!(ins.modulation.macro_name(3), "Macro 4");
        let back: crate::Insert = serde_json::from_str(&serde_json::to_string(&ins).unwrap()).unwrap();
        assert_eq!(back.modulation, ins.modulation);
    }
}
