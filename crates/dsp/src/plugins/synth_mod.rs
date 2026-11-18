//! The modulation matrix the three synths share (Subtractive, Wavetable, FM): up to
//! [`MAX_SLOTS`] slots of source -> destination with a bipolar amount, and four macros, each a
//! parameter (`macro1`..`macro4`) driving up to [`MAX_MACRO_TARGETS`] destinations over a range.
//!
//! What a session stores (`Insert::modulation`) is names; the engine turns it into numbers off
//! the audio thread ([`compile`]) and the mix engine hands them over between blocks
//! (`Plugin::set_prepared_state`), which only copies them into the fixed storage of a
//! [`ModMatrix`]: no allocation, locking or panic on the audio thread.
//!
//! Units: an amount is a share of the destination knob's travel (its 0..1 position, honouring
//! its taper), so a slot of 0.25 from a 0..1 source moves the knob's value over a quarter of its
//! sweep. A synth evaluates every destination per voice every [`TICK`] samples and ramps to the
//! next value linearly (no zipper); with nothing patched it runs exactly the code it ran before.

use crate::{ParamInfo, Taper, Unit};

/// Slots per synth.
pub const MAX_SLOTS: usize = 16;
/// Macros per synth (parameters `macro1`..`macro4`).
pub const MAX_MACROS: usize = 4;
/// Destinations one macro drives.
pub const MAX_MACRO_TARGETS: usize = 8;
/// Most destinations a synth offers (its table's length).
pub(crate) const MAX_DESTS: usize = 32;
/// Samples between two evaluations of the matrix (the values ramp linearly between them).
pub(crate) const TICK: usize = 16;
/// How far modulation may lift the `level` destination above its knob, dB.
pub const LEVEL_LIFT_DB: f32 = 6.0;

/// A modulation source.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct ModSource {
    pub id: &'static str,
    pub name: &'static str,
    /// The range the source moves over.
    pub lo: f32,
    pub hi: f32,
    /// One value per voice (else one for the whole synth).
    pub per_voice: bool,
}

pub(crate) const SRC_AMP: usize = 0;
pub(crate) const SRC_ENV2: usize = 1;
pub(crate) const SRC_LFO: usize = 2;
pub(crate) const SRC_VEL: usize = 3;
pub(crate) const SRC_WHEEL: usize = 4;
pub(crate) const SRC_KEY: usize = 5;
pub(crate) const SRC_RND: usize = 6;
/// Sources, by index.
pub const N_SOURCES: usize = 7;

/// The sources, in the order the matrix numbers them.
pub static SOURCES: [ModSource; N_SOURCES] = [
    ModSource { id: "amp_env", name: "Amp Env", lo: 0.0, hi: 1.0, per_voice: true },
    ModSource { id: "env2", name: "Env 2", lo: 0.0, hi: 1.0, per_voice: true },
    ModSource { id: "lfo", name: "LFO", lo: -1.0, hi: 1.0, per_voice: false },
    ModSource { id: "velocity", name: "Velocity", lo: 0.0, hi: 1.0, per_voice: true },
    ModSource { id: "mod_wheel", name: "Mod Wheel", lo: 0.0, hi: 1.0, per_voice: false },
    ModSource { id: "key", name: "Key", lo: -1.0, hi: 1.0, per_voice: true },
    ModSource { id: "random", name: "Random", lo: -1.0, hi: 1.0, per_voice: true },
];

/// The source `id` names (its own id, or one of the short names the panels use: `env1`, `vel`,
/// `wheel`, `rnd`), by index.
pub fn source_index(id: &str) -> Option<usize> {
    let id = id.trim();
    let canon = match id.to_ascii_lowercase().as_str() {
        "env1" | "amp" => "amp_env",
        "vel" => "velocity",
        "wheel" | "modwheel" => "mod_wheel",
        "rnd" => "random",
        _ => return SOURCES.iter().position(|s| s.id.eq_ignore_ascii_case(id)),
    };
    SOURCES.iter().position(|s| s.id == canon)
}

/// The id of the virtual destination on synths with no master pitch knob: ±24 semitones around
/// the played note.
pub const PITCH: &str = "pitch";

/// The virtual pitch destination's range.
pub static PITCH_INFO: ParamInfo = ParamInfo {
    id: PITCH,
    name: "Pitch",
    min: -24.0,
    max: 24.0,
    default: 0.0,
    unit: Unit::Semitones,
    taper: Taper::Linear,
    choices: &[],
    display: None,
    group: "",
};

/// The destination shared by every voice (only shared sources and macros can drive it).
pub const SHARED_DEST: &str = "lfo_rate";

/// The macro parameters' ids.
pub const MACRO_PARAMS: [&str; MAX_MACROS] = ["macro1", "macro2", "macro3", "macro4"];

/// The destinations `plugin` offers (parameter ids, and `pitch` where it has no pitch knob), or
/// none for a plugin without the matrix.
pub fn dests(plugin: &str) -> &'static [&'static str] {
    match plugin {
        "subtractive_synth" => &super::synth::MOD_DESTS,
        "wavetable_synth" => &super::wavetable::MOD_DESTS,
        "fm_synth" => &super::fm::MOD_DESTS,
        _ => &[],
    }
}

/// Whether `plugin` has the matrix.
pub fn has_matrix(plugin: &str) -> bool {
    !dests(plugin).is_empty()
}

/// Destination `id`'s parameter description on `plugin` (the virtual pitch's own for `pitch`).
pub fn dest_info(plugin: &str, id: &str) -> Option<&'static ParamInfo> {
    if !dests(plugin).contains(&id) {
        return None;
    }
    if id == PITCH { Some(&PITCH_INFO) } else { crate::plugin_info(plugin)?.param(id) }
}

/// Whether destination `id` is evaluated per voice (else once for the synth).
pub fn dest_per_voice(id: &str) -> bool {
    id != SHARED_DEST
}

/// One slot as the session names it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlotSpec<'a> {
    pub source: &'a str,
    pub dest: &'a str,
    pub amount: f32,
}

/// One macro target as the session names it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetSpec<'a> {
    /// The macro, 0..4.
    pub macro_index: usize,
    pub dest: &'a str,
    pub from: f32,
    pub to: f32,
}

/// Why a slot cannot be made.
pub fn check_slot(plugin: &str, source: &str, dest: &str) -> Result<(usize, usize), String> {
    let list = dests(plugin);
    if list.is_empty() {
        return Err(format!("`{plugin}` has no modulation matrix (the Subtractive, Wavetable and FM synths do)"));
    }
    let s = source_index(source)
        .ok_or_else(|| format!("no source `{source}` (one of: {})", SOURCES.iter().map(|s| s.id).collect::<Vec<_>>().join(", ")))?;
    let d = list.iter().position(|d| *d == dest).ok_or_else(|| format!("`{plugin}` cannot modulate `{dest}` (one of: {})", list.join(", ")))?;
    let src = SOURCES.get(s).ok_or_else(|| format!("no source `{source}`"))?;
    if src.per_voice && !dest_per_voice(dest) {
        return Err(format!("`{dest}` is shared by every voice: only the LFO, the mod wheel or a macro can move it, not {}", src.name));
    }
    Ok((s, d))
}

/// A bipolar amount kept in range (NaN: 0).
pub fn clamp_amount(x: f32) -> f32 {
    if x.is_finite() { x.clamp(-1.0, 1.0) } else { 0.0 }
}

/// The matrix as numbers for [`ModMatrix::load`] (made off the audio thread). Unusable entries
/// (unknown names, a per-voice source on a shared destination, past the limits) are left out.
/// `None` when nothing is patched: the synth then runs as it always has.
pub fn compile(plugin: &str, slots: &[SlotSpec<'_>], targets: &[TargetSpec<'_>]) -> Option<Vec<f64>> {
    let mut out = vec![1.0, 0.0];
    let mut n = 0usize;
    for s in slots.iter().take(MAX_SLOTS) {
        if let Ok((src, d)) = check_slot(plugin, s.source, s.dest) {
            out.extend([src as f64, d as f64, f64::from(clamp_amount(s.amount))]);
            n += 1;
        }
    }
    if let Some(c) = out.get_mut(1) {
        *c = n as f64;
    }
    let at = out.len();
    out.push(0.0);
    let mut per_macro = [0usize; MAX_MACROS];
    let mut nt = 0usize;
    for t in targets {
        let Some(count) = per_macro.get_mut(t.macro_index) else { continue };
        if *count >= MAX_MACRO_TARGETS {
            continue;
        }
        let Some(d) = dests(plugin).iter().position(|d| *d == t.dest) else { continue };
        *count += 1;
        out.extend([t.macro_index as f64, d as f64, f64::from(clamp_amount(t.from)), f64::from(clamp_amount(t.to))]);
        nt += 1;
    }
    if let Some(c) = out.get_mut(at) {
        *c = nt as f64;
    }
    (n + nt > 0).then_some(out)
}

#[derive(Debug, Clone, Copy, Default)]
struct CSlot {
    src: u8,
    dest: u8,
    amount: f32,
}

#[derive(Debug, Clone, Copy, Default)]
struct CTarget {
    mac: u8,
    dest: u8,
    from: f32,
    to: f32,
}

/// A synth's matrix, ready to run: fixed storage, filled by [`ModMatrix::load`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct ModMatrix {
    slots: [CSlot; MAX_SLOTS],
    n: usize,
    targets: [CTarget; MAX_MACROS * MAX_MACRO_TARGETS],
    nt: usize,
    /// The destinations patched (each once), and the same as a lookup.
    used: [u8; MAX_DESTS],
    n_used: usize,
    on: [bool; MAX_DESTS],
}

impl ModMatrix {
    pub(crate) const fn new() -> Self {
        ModMatrix {
            slots: [CSlot { src: 0, dest: 0, amount: 0.0 }; MAX_SLOTS],
            n: 0,
            targets: [CTarget { mac: 0, dest: 0, from: 0.0, to: 0.0 }; MAX_MACROS * MAX_MACRO_TARGETS],
            nt: 0,
            used: [0; MAX_DESTS],
            n_used: 0,
            on: [false; MAX_DESTS],
        }
    }

    /// Takes [`compile`]'s numbers for a synth with `n_dests` destinations (empty: nothing
    /// patched). Hostile input: anything out of range is skipped. Copies only.
    pub(crate) fn load(&mut self, data: &[f64], n_dests: usize) {
        *self = ModMatrix::new();
        let n_dests = n_dests.min(MAX_DESTS);
        let idx = |x: Option<&f64>, max: usize| x.copied().filter(|v| v.is_finite() && *v >= 0.0 && *v < max as f64).map(|v| v as usize);
        let amt = |x: Option<&f64>| x.copied().filter(|v| v.is_finite()).map_or(0.0, |v| clamp_amount(v as f32));
        if data.first().copied() != Some(1.0) {
            return;
        }
        let n = idx(data.get(1), MAX_SLOTS + 1).unwrap_or(0);
        let mut at = 2;
        for _ in 0..n {
            let (s, d, a) = (idx(data.get(at), N_SOURCES), idx(data.get(at + 1), n_dests), amt(data.get(at + 2)));
            at += 3;
            if let (Some(s), Some(d)) = (s, d)
                && let Some(slot) = self.slots.get_mut(self.n)
            {
                *slot = CSlot { src: s as u8, dest: d as u8, amount: a };
                self.n += 1;
                self.mark(d);
            }
        }
        let nt = idx(data.get(at), MAX_MACROS * MAX_MACRO_TARGETS + 1).unwrap_or(0);
        at += 1;
        for _ in 0..nt {
            let (m, d, f, t) = (idx(data.get(at), MAX_MACROS), idx(data.get(at + 1), n_dests), amt(data.get(at + 2)), amt(data.get(at + 3)));
            at += 4;
            if let (Some(m), Some(d)) = (m, d)
                && let Some(slot) = self.targets.get_mut(self.nt)
            {
                *slot = CTarget { mac: m as u8, dest: d as u8, from: f, to: t };
                self.nt += 1;
                self.mark(d);
            }
        }
    }

    fn mark(&mut self, d: usize) {
        if let Some(on) = self.on.get_mut(d)
            && !*on
        {
            *on = true;
            if let Some(u) = self.used.get_mut(self.n_used) {
                *u = d as u8;
                self.n_used += 1;
            }
        }
    }

    /// Anything patched.
    #[inline]
    pub(crate) fn active(&self) -> bool {
        self.n_used > 0
    }

    /// Destination `d` patched.
    #[inline]
    pub(crate) fn on(&self, d: usize) -> bool {
        self.on.get(d).copied().unwrap_or(false)
    }

    /// The destinations patched.
    #[inline]
    pub(crate) fn used(&self) -> &[u8] {
        self.used.get(..self.n_used).unwrap_or(&[])
    }

    /// The macros' offsets per destination at macro values `values` (0..1 each).
    pub(crate) fn macro_offsets(&self, values: [f32; MAX_MACROS], out: &mut [f32; MAX_DESTS]) {
        *out = [0.0; MAX_DESTS];
        for t in self.targets.iter().take(self.nt) {
            let v = values.get(usize::from(t.mac)).copied().unwrap_or(0.0);
            if let Some(o) = out.get_mut(usize::from(t.dest)) {
                *o += t.from + (t.to - t.from) * v;
            }
        }
    }

    /// Adds the slots' offsets per destination for source values `src` into `out`.
    #[inline]
    pub(crate) fn add_offsets(&self, src: &[f32; N_SOURCES], out: &mut [f32; MAX_DESTS]) {
        for s in self.slots.iter().take(self.n) {
            let x = src.get(usize::from(s.src)).copied().unwrap_or(0.0);
            if let Some(o) = out.get_mut(usize::from(s.dest)) {
                *o += x * s.amount;
            }
        }
    }
}

/// A voice's modulated values: per destination the value now (in the parameter's own units) and
/// the step it ramps by each sample to the next evaluation.
#[derive(Debug, Clone, Copy)]
pub(crate) struct VoiceMod {
    pub(crate) val: [f32; MAX_DESTS],
    step: [f32; MAX_DESTS],
    /// The last evaluation's values in the parameters' own units (before the synth's
    /// conversion), for what a synth works out once per evaluation.
    target: [f32; MAX_DESTS],
    /// This note's random value (-1..1).
    pub(crate) rnd: f32,
    /// Just started: the next evaluation jumps to its values instead of ramping.
    pub(crate) fresh: bool,
}

impl VoiceMod {
    pub(crate) const fn new() -> Self {
        VoiceMod { val: [0.0; MAX_DESTS], step: [0.0; MAX_DESTS], target: [0.0; MAX_DESTS], rnd: 0.0, fresh: true }
    }

    /// Destination `d`'s value at the last evaluation, in the parameter's own units.
    #[inline]
    pub(crate) fn target(&self, d: usize) -> f32 {
        self.target.get(d).copied().unwrap_or(0.0)
    }

    /// Destination `d` heads for `target`, reached in `ticks` samples (at once when fresh).
    #[inline]
    pub(crate) fn aim(&mut self, d: usize, target: f32, ticks: usize) {
        let (Some(v), Some(s)) = (self.val.get_mut(d), self.step.get_mut(d)) else { return };
        let target = if target.is_finite() { target } else { *v };
        if self.fresh {
            *v = target;
            *s = 0.0;
        } else {
            *s = (target - *v) / ticks.max(1) as f32;
        }
    }

    /// One sample along the ramps of the destinations `used`.
    #[inline]
    pub(crate) fn advance(&mut self, used: &[u8]) {
        for &d in used {
            let d = usize::from(d);
            if let (Some(v), Some(s)) = (self.val.get_mut(d), self.step.get(d)) {
                *v += *s;
            }
        }
    }

    /// The value of destination `d` now.
    #[inline]
    pub(crate) fn get(&self, d: usize) -> f32 {
        self.val.get(d).copied().unwrap_or(0.0)
    }
}

/// The parameter descriptions of `plugin`'s destinations by index (the virtual pitch's where a
/// table entry is unknown, which only a table/registry mismatch could cause).
pub(crate) fn infos(plugin: &str) -> [&'static ParamInfo; MAX_DESTS] {
    let mut out = [&PITCH_INFO; MAX_DESTS];
    for (o, d) in out.iter_mut().zip(dests(plugin)) {
        if let Some(i) = dest_info(plugin, d) {
            *o = i;
        }
    }
    out
}

/// Aims every patched per-voice destination of one voice (`m`) at its value for sources `src`:
/// the macros' offsets plus the slots', on `bases` (the knobs now), then `derive(d, value, base)`
/// into the units the synth uses it in. `shared` (the destination evaluated for the whole synth)
/// is left alone.
#[inline]
pub(crate) fn aim_voice(
    mm: &ModMatrix,
    m: &mut VoiceMod,
    src: &[f32; N_SOURCES],
    macro_off: &[f32; MAX_DESTS],
    bases: &[f32; MAX_DESTS],
    infos: &[&'static ParamInfo; MAX_DESTS],
    shared: usize,
    ticks: usize,
    derive: impl Fn(usize, f32, f32) -> f32,
) {
    let mut off = *macro_off;
    mm.add_offsets(src, &mut off);
    for &d in mm.used() {
        let d = usize::from(d);
        if d == shared {
            continue;
        }
        let (Some(info), Some(&base), Some(&o)) = (infos.get(d), bases.get(d), off.get(d)) else { continue };
        let v = modded(info, base, o);
        m.aim(d, derive(d, v, base), ticks);
        if let Some(t) = m.target.get_mut(d) {
            *t = v;
        }
    }
    m.fresh = false;
}

/// The shared destination's value: the macros' offsets plus the slots' from the shared sources
/// (`lfo`, `wheel`) on `base`.
pub(crate) fn shared_value(mm: &ModMatrix, d: usize, lfo: f32, wheel: f32, macro_off: &[f32; MAX_DESTS], info: &ParamInfo, base: f32) -> f32 {
    let mut src = [0.0f32; N_SOURCES];
    src[SRC_LFO] = lfo;
    src[SRC_WHEEL] = wheel;
    let mut off = *macro_off;
    mm.add_offsets(&src, &mut off);
    modded(info, base, off.get(d).copied().unwrap_or(0.0))
}

/// `base` moved by `off` of `info`'s travel, kept in its range.
#[inline]
pub(crate) fn modded(info: &ParamInfo, base: f32, off: f32) -> f32 {
    if off == 0.0 {
        return info.clamp(base);
    }
    info.from_normalized(info.to_normalized(base) + off)
}

/// The gain the `level` destination gives a voice: its modulated value `level` (dB) against the
/// knob's `base`, lifted at most [`LEVEL_LIFT_DB`] and never past the knob's top (+6 dB).
#[inline]
pub(crate) fn level_gain(level: f32, base: f32) -> f32 {
    crate::db_to_gain(level.min(base + LEVEL_LIFT_DB).min(6.0) - base)
}

/// The next per-note random value (-1..1) from `state` (xorshift; never 0).
#[inline]
pub(crate) fn next_random(state: &mut u32) -> f32 {
    let mut x = if *state == 0 { 0x9E37_79B9 } else { *state };
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    (x as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// The seed the per-note random values start from after a reset.
pub(crate) const RANDOM_SEED: u32 = 0x5EED_0C0A;

/// A key's place for the `key` source: (note - 60) / 60, clamped to -1..1.
#[inline]
pub(crate) fn key_value(note: u8) -> f32 {
    ((f32::from(note) - 60.0) / 60.0).clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_and_load_round_trip_and_skip_what_does_not_fit() {
        let slots = [
            SlotSpec { source: "lfo", dest: "cutoff", amount: 0.5 },
            SlotSpec { source: "nope", dest: "cutoff", amount: 0.5 },
            SlotSpec { source: "vel", dest: "level", amount: 9.0 },
            SlotSpec { source: "env2", dest: "lfo_rate", amount: 0.5 },
        ];
        let targets = [
            TargetSpec { macro_index: 1, dest: "resonance", from: 0.0, to: 0.4 },
            TargetSpec { macro_index: 7, dest: "resonance", from: 0.0, to: 1.0 },
        ];
        let data = compile("subtractive_synth", &slots, &targets).unwrap();
        let mut m = ModMatrix::new();
        m.load(&data, dests("subtractive_synth").len());
        assert_eq!((m.n, m.nt), (2, 1));
        assert!((m.slots[1].amount - 1.0).abs() < 1e-6, "amount clamped");
        let cut = dests("subtractive_synth").iter().position(|d| *d == "cutoff").unwrap();
        assert!(m.on(cut) && m.active());
        assert!(compile("subtractive_synth", &[], &[]).is_none());
        assert!(compile("compressor", &slots, &targets).is_none());
        // Hostile numbers never panic and load nothing usable.
        for junk in [vec![], vec![1.0, 1e9], vec![1.0, 3.0, f64::NAN, 1.0], vec![2.0, 1.0, 0.0, 0.0, 0.5], vec![1.0, 1.0, 0.0, 999.0, 0.5, 1.0]] {
            let mut m = ModMatrix::new();
            m.load(&junk, 19);
            assert!(m.n <= 1);
        }
    }

    #[test]
    fn checks_name_the_valid_choices() {
        assert!(check_slot("subtractive_synth", "LFO", "cutoff").is_ok());
        assert!(check_slot("subtractive_synth", "amp_env", "lfo_rate").unwrap_err().contains("shared"));
        assert!(check_slot("subtractive_synth", "lfo", "osc1_wave").unwrap_err().contains("cutoff"));
        assert!(check_slot("eq7", "lfo", "cutoff").is_err());
        assert_eq!(source_index("rnd"), Some(SRC_RND));
        for p in ["subtractive_synth", "wavetable_synth", "fm_synth"] {
            assert!(dests(p).len() <= MAX_DESTS, "{p}");
            for d in dests(p) {
                let info = dest_info(p, d).unwrap_or_else(|| panic!("{p}: {d}"));
                assert!(info.unit != Unit::Choice && info.unit != Unit::Toggle, "{p}: {d}");
            }
        }
    }

    const SYNTHS: [&str; 3] = ["subtractive_synth", "wavetable_synth", "fm_synth"];

    /// The golden phrase through `id` with `slots`/`targets` patched (none: no matrix at all)
    /// and `set` parameters.
    fn phrase(id: &str, patch: Option<(&[SlotSpec<'_>], &[TargetSpec<'_>])>, set: &[(&str, f32)]) -> Vec<f32> {
        let mut p = crate::create(id).unwrap();
        for (k, v) in set {
            assert!(p.set_param(k, *v), "{k}");
        }
        if let Some((s, t)) = patch {
            p.set_prepared_state(&compile(id, s, t).unwrap_or_default());
        }
        super::super::golden::fingerprint(p.as_mut())
    }

    /// Raw output of `id` playing `notes` (offset, note, velocity) for `frames`, left channel.
    fn render(
        id: &str,
        slots: &[SlotSpec<'_>],
        targets: &[TargetSpec<'_>],
        set: &[(&str, f32)],
        notes: &[(usize, u8, u8)],
        frames: usize,
    ) -> Vec<f32> {
        let mut p = crate::create(id).unwrap();
        p.prepare(48_000.0, 256, 2);
        for (k, v) in set {
            assert!(p.set_param(k, *v), "{k}");
        }
        p.set_prepared_state(&compile(id, slots, targets).unwrap_or_default());
        let mut io = vec![vec![0.0f32; 256]; 2];
        let mut out = Vec::with_capacity(frames);
        let mut at = 0;
        while at < frames {
            for (w, n, v) in notes.iter().filter(|(w, _, _)| (at..at + 256).contains(w)) {
                p.note_on(w - at, *n, *v);
            }
            p.process(&mut io, 256);
            out.extend_from_slice(&io[0]);
            at += 256;
        }
        out
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// Semi-modular: with nothing patched (no matrix, an empty one, or one loaded and cleared)
    /// every synth renders exactly as it did, bit for bit.
    #[test]
    fn an_empty_matrix_is_bit_identical_to_none() {
        for id in SYNTHS {
            let none = phrase(id, None, &[]);
            assert_eq!(none, phrase(id, Some((&[], &[])), &[]), "{id}: empty");
            let mut p = crate::create(id).unwrap();
            let cut = if id == "fm_synth" { "op2_level" } else { "cutoff" };
            p.set_prepared_state(&compile(id, &[SlotSpec { source: "lfo", dest: cut, amount: 0.5 }], &[]).unwrap());
            p.set_prepared_state(&[]);
            assert_eq!(none, super::super::golden::fingerprint(p.as_mut()), "{id}: patched, then cleared");
            assert_ne!(none, phrase(id, Some((&[SlotSpec { source: "lfo", dest: cut, amount: 0.5 }], &[])), &[]), "{id}: a slot is heard");
        }
    }

    /// Velocity to level: a soft note and a hard one differ more with the slot than without.
    #[test]
    fn velocity_to_level_and_the_macros_move_the_sound() {
        for id in SYNTHS {
            let soft_hard = |slots: &[SlotSpec<'_>]| {
                let a = rms(&render(id, slots, &[], &[], &[(0, 60, 30)], 12_000)[2_000..]);
                let b = rms(&render(id, slots, &[], &[], &[(0, 60, 127)], 12_000)[2_000..]);
                b / a
            };
            let plain = soft_hard(&[]);
            let patched = soft_hard(&[SlotSpec { source: "velocity", dest: "level", amount: 0.3 }]);
            assert!(patched > plain * 1.2, "{id}: {patched} against {plain}");
            // A macro on the level: at 0 its `from`, at 100 % its `to`.
            let t = [TargetSpec { macro_index: 2, dest: "level", from: 0.0, to: -0.4 }];
            let at = |m: f32| rms(&render(id, &[], &t, &[("macro3", m)], &[(0, 60, 100)], 12_000)[2_000..]);
            let (lo, hi) = (at(100.0), at(0.0));
            let base = rms(&render(id, &[], &[], &[], &[(0, 60, 100)], 12_000)[2_000..]);
            assert!((hi / base - 1.0).abs() < 0.02, "{id}: macro at 0 changes nothing ({hi} against {base})");
            assert!(lo < hi * 0.3, "{id}: macro at full turns it down ({lo} against {hi})");
        }
    }

    /// The level destination never lifts a voice more than 6 dB, whatever the amounts.
    #[test]
    fn the_level_destination_is_capped() {
        for id in SYNTHS {
            let base = rms(&render(id, &[], &[], &[("level", -20.0)], &[(0, 60, 127)], 12_000)[2_000..]);
            let slots = [
                SlotSpec { source: "velocity", dest: "level", amount: 1.0 },
                SlotSpec { source: "amp_env", dest: "level", amount: 1.0 },
                SlotSpec { source: "key", dest: "level", amount: 1.0 },
            ];
            let t = [TargetSpec { macro_index: 0, dest: "level", from: 1.0, to: 1.0 }];
            let up = rms(&render(id, &slots, &t, &[("level", -20.0)], &[(0, 60, 127)], 12_000)[2_000..]);
            assert!(20.0 * (up / base).log10() < 6.05, "{id}: +{} dB", 20.0 * (up / base).log10());
        }
    }

    /// Random: each note a different value, the same ones again after a reset (a bounce
    /// repeats); key: a higher note moves further.
    #[test]
    fn random_repeats_after_a_reset_and_key_follows_the_note() {
        for id in SYNTHS {
            let pitch = if id == "wavetable_synth" { "osc_pitch" } else { PITCH };
            let slots = [SlotSpec { source: "random", dest: pitch, amount: 0.1 }];
            let a = render(id, &slots, &[], &[], &[(0, 60, 100), (6_000, 60, 100)], 12_000);
            let b = render(id, &slots, &[], &[], &[(0, 60, 100), (6_000, 60, 100)], 12_000);
            assert_eq!(a, b, "{id}: the same after a fresh start");
            let plain = render(id, &[], &[], &[], &[(0, 60, 100), (6_000, 60, 100)], 12_000);
            assert_ne!(a, plain, "{id}");
            assert!(a.iter().all(|x| x.is_finite() && x.abs() < 4.0), "{id}");
        }
    }

    /// Every source to every destination, at full amounts both ways, with notes across the
    /// keyboard, the wheel and the macros up: nothing goes non-finite or wild.
    #[test]
    fn every_source_to_every_destination_stays_finite() {
        for id in SYNTHS {
            for amount in [1.0f32, -1.0] {
                for d in dests(id) {
                    let slots: Vec<SlotSpec<'_>> =
                        SOURCES.iter().filter(|s| !(s.per_voice && !dest_per_voice(d))).map(|s| SlotSpec { source: s.id, dest: d, amount }).collect();
                    let t = [TargetSpec { macro_index: 0, dest: d, from: -amount, to: amount }];
                    let mut p = crate::create(id).unwrap();
                    p.prepare(48_000.0, 256, 2);
                    p.set_param("macro1", 70.0);
                    p.set_param("unison", 3.0);
                    p.set_prepared_state(&compile(id, &slots, &t).unwrap());
                    let mut io = vec![vec![0.0f32; 256]; 2];
                    p.midi(0, [0xB0, 1, 127]);
                    for (i, n) in [24u8, 60, 108].iter().enumerate() {
                        p.note_on(i * 50, *n, 127);
                    }
                    for _ in 0..40 {
                        p.process(&mut io, 256);
                        assert!(io.iter().flatten().all(|x| x.is_finite() && x.abs() < 16.0), "{id}: {d} {amount}");
                    }
                }
            }
        }
    }

    /// No zipper: the LFO sweeping the level of a held sine moves it in small steps (the ramp
    /// between evaluations), never a jump of a whole evaluation's worth at once.
    #[test]
    fn modulation_ramps_between_evaluations() {
        let slots = [SlotSpec { source: "lfo", dest: "level", amount: -0.5 }];
        let set =
            [("osc1_wave", 2.0), ("osc_mix", 0.0), ("cutoff", 20_000.0), ("amp_sustain", 100.0), ("lfo_rate", 20.0), ("filter_env_amount", 0.0)];
        let x = render("subtractive_synth", &slots, &[], &set, &[(0, 69, 100)], 24_000);
        // The envelope of a 440 Hz sine: the peak of each half cycle; a step shows as a sudden
        // change of the level from one sample to the next, beyond what the sine does.
        let worst = x.windows(2).skip(4_000).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        let peak = x.iter().skip(4_000).fold(0.0f32, |m, v| m.max(v.abs()));
        // A 440 Hz sine moves at most 2π·440/48000 ≈ 0.058 of its peak per sample.
        assert!(worst < peak * 0.07, "{worst} against {peak}");
    }

    /// The matrix's cost: 16 voices with 16 slots against none (printed; run with
    /// `--ignored --nocapture`).
    #[test]
    #[ignore]
    fn matrix_cost() {
        for id in SYNTHS {
            let ds = dests(id);
            let srcs = ["amp_env", "env2", "lfo", "velocity", "mod_wheel", "key", "random"];
            let slots: Vec<SlotSpec<'_>> = (0..MAX_SLOTS)
                .map(|i| {
                    let d = ds[(i * 5 + 1) % ds.len()];
                    let d = if d == SHARED_DEST { "level" } else { d };
                    SlotSpec { source: srcs[i % srcs.len()], dest: d, amount: 0.1 }
                })
                .collect();
            let time = |patched: bool| {
                let mut p = crate::create(id).unwrap();
                p.prepare(48_000.0, 256, 2);
                if patched {
                    p.set_prepared_state(&compile(id, &slots, &[]).unwrap());
                }
                let mut io = vec![vec![0.0f32; 256]; 2];
                for n in 0..16u8 {
                    p.note_on(0, 40 + n * 3, 100);
                }
                let t = std::time::Instant::now();
                for _ in 0..(48_000 * 4 / 256) {
                    p.process(&mut io, 256);
                }
                t.elapsed().as_secs_f64() / 4.0
            };
            let (a, b) = (time(false), time(true));
            println!("{id}: 16 voices, 1 s of audio: {:.1} ms plain, {:.1} ms with 16 slots (+{:.0} %)", a * 1e3, b * 1e3, (b / a - 1.0) * 100.0);
        }
    }

    #[test]
    fn the_level_is_lifted_at_most_six_db() {
        let info = crate::plugin_info("subtractive_synth").unwrap().param("level").unwrap();
        let db = |g: f32| 20.0 * g.log10();
        assert!((db(level_gain(modded(info, -20.0, 1.0), -20.0)) - 6.0).abs() < 1e-3);
        assert!((db(level_gain(modded(info, 3.0, 1.0), 3.0)) - 3.0).abs() < 1e-3);
        assert!(db(level_gain(modded(info, -6.0, -1.0), -6.0)) <= -41.9);
    }
}
