//! The built-in synths' modulation patching (the Subtractive, Wavetable and FM synths): the
//! matrix's slots and the macros' names and targets, kept on the instrument
//! (`Insert::modulation`) and made ready for the audio thread here, off it ([`compile`] into
//! `Insert::prepared`), which the mix engine hands over between blocks.

use soundcraft_dsp::plugins::synth_mod::{self as sm, SlotSpec, TargetSpec};
use soundcraft_model::Insert;

/// `ins`'s modulation patch as the numbers its synth takes (`None`: nothing patched).
pub(crate) fn compile(ins: &Insert) -> Option<Vec<f64>> {
    let m = &ins.modulation;
    let slots: Vec<SlotSpec<'_>> = m.slots.iter().map(|s| SlotSpec { source: &s.source, dest: &s.dest, amount: s.amount }).collect();
    let targets: Vec<TargetSpec<'_>> = m
        .macros
        .iter()
        .enumerate()
        .flat_map(|(i, mac)| mac.targets.iter().map(move |t| TargetSpec { macro_index: i, dest: &t.dest, from: t.from, to: t.to }))
        .collect();
    sm::compile(&ins.plugin, &slots, &targets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ModSlot, SynthMod};

    #[test]
    fn a_patched_synth_is_made_ready_and_an_empty_one_is_not() {
        let mut ins = Insert::new("subtractive_synth");
        super::super::module_ceiling::prepare_insert(&mut ins);
        assert!(ins.prepared.is_none());
        ins.modulation = SynthMod { slots: vec![ModSlot { source: "lfo".into(), dest: "cutoff".into(), amount: 0.2 }], macros: vec![] };
        super::super::module_ceiling::prepare_insert(&mut ins);
        assert!(ins.prepared.as_ref().is_some_and(|p| p.data.len() > 3));
    }
}
