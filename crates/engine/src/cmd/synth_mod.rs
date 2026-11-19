//! The built-in synths' modulation patching (the Subtractive, Wavetable and FM synths): the
//! matrix's slots and the macros' names and targets, kept on the instrument
//! (`Insert::modulation`) and made ready for the audio thread here, off it ([`compile`] into
//! `Insert::prepared`), which the mix engine hands over between blocks.
//!
//! Every edit is one undo step (a knob drag on an amount goes through `execute_merged`); values
//! are clamped; names that mean nothing are errors that list what would. The macros' values are
//! the synths' own parameters (`macro1`..`macro4`, `mix.instrument_param`).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_dsp::plugins::synth_mod::{self as sm, SlotSpec, TargetSpec};
use soundcraft_model::{Insert, MAX_MACRO_NAME, MAX_MACRO_TARGETS, MAX_MACROS, MAX_MOD_SLOTS, MacroTarget, ModSlot, SynthMacro, SynthMod};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.mod_add",
            "Add Modulation",
            [],
            None,
            "{track?, module? (\"instrument\" or a module id), source, dest, amount? (-1..1 of the knob's travel, default 0.25)} → {slot, slots}: a modulation-matrix slot on a built-in synth (Subtractive, Wavetable, FM); at most 16. Sources: amp_env, env2, lfo, velocity, mod_wheel, key, random; dests: see mix.mod_info",
            has_selection,
            mod_add
        ),
        cmd!(
            "mix.mod_set",
            "Set Modulation",
            [],
            None,
            "{track?, module?, slot: 0..15, source?, dest?, amount?} → {slot}: changes one slot of a synth's modulation matrix",
            has_selection,
            mod_set
        ),
        cmd!(
            "mix.mod_remove",
            "Remove Modulation",
            [],
            None,
            "{track?, module?, slot: 0..15 | \"all\"} → {slots}: removes a slot (later ones move up) or every slot",
            has_selection,
            mod_remove
        ),
        cmd!(
            "mix.macro_assign",
            "Assign Macro",
            [],
            None,
            "{track?, module?, macro: 1..4, dest, amount? | from?, to? (-1..1 each), remove?: bool} → {macro, targets}: a synth macro drives `dest` from `from` (macro at 0) to `to` (at 100 %); `amount` = from 0 to amount; at most 8 per macro; one already on `dest` is changed",
            has_selection,
            macro_assign
        ),
        cmd!(
            "mix.macro_name",
            "Rename Macro",
            [],
            None,
            "{track?, module?, macro: 1..4, name} → {macro, name}: a synth macro's name (at most 24 characters; empty: \"Macro n\")",
            has_selection,
            macro_name
        ),
        cmd!(
            "mix.mod_surprise",
            "Surprise",
            [],
            None,
            "{track?, module?, seed?: integer} → {added: [slot], slots}: two or three random modulation slots in safe ranges (pitch within a fifth, level at most +3 dB), the same for the same seed",
            has_selection,
            mod_surprise
        ),
        cmd!(query
            "mix.mod_info",
            "Modulation Info",
            [],
            None,
            "{track?, module?} → {plugin, sources, dests, slots, macros, arcs}: a synth's modulation patch, what it can use, and per destination the range its knob is moved over (`lo`/`hi`, in knob travel, with each slot's share)",
            has_selection,
            mod_info
        ),
    ]
}

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

/// The synth a command names: the track's instrument (`module` absent or "instrument") or a
/// chain module, checked to have the matrix.
fn synth_mut<'a>(e: &'a mut Engine, c: &str, p: &Value) -> Result<&'a mut Insert> {
    let t = super::module::first_track(e, c, p)?;
    let instrument = match p.get("module") {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) if s.trim().eq_ignore_ascii_case("instrument") => true,
        Some(Value::String(_)) => return Err(bad(c, "`module` must be \"instrument\" or a module id")),
        Some(_) => false,
    };
    let ins = if instrument {
        e.session_mut().track_mut(t).and_then(|tr| tr.instrument.as_mut()).ok_or_else(|| bad(c, "the track has no instrument"))?
    } else {
        let id = super::module::module_id(c, p)?;
        super::module::find_mut(e, c, t, id)?
    };
    if !sm::has_matrix(&ins.plugin) {
        return Err(bad(c, format!("`{}` has no modulation matrix (the Subtractive, Wavetable and FM synths do)", ins.plugin)));
    }
    Ok(ins)
}

/// Read-only twin of [`synth_mut`].
fn synth_ref<'a>(e: &'a Engine, c: &str, p: &Value) -> Result<&'a Insert> {
    let t = super::module::first_track(e, c, p)?;
    let tr = e.session().track(t).ok_or_else(|| bad(c, "no track"))?;
    let ins = match p.get("module") {
        None | Some(Value::Null) => tr.instrument.as_ref(),
        Some(Value::String(s)) if s.trim().eq_ignore_ascii_case("instrument") => tr.instrument.as_ref(),
        Some(_) => {
            let id = super::module::module_id(c, p)?;
            tr.mixer.inserts.iter().find(|i| i.id == id)
        }
    }
    .ok_or_else(|| bad(c, "no such instrument or module"))?;
    if !sm::has_matrix(&ins.plugin) {
        return Err(bad(c, format!("`{}` has no modulation matrix (the Subtractive, Wavetable and FM synths do)", ins.plugin)));
    }
    Ok(ins)
}

/// An amount field: a finite number, clamped to -1..1.
fn amount_of(c: &str, p: &Value, key: &str) -> Result<Option<f32>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_f64()
            .filter(|x| x.is_finite())
            .map(|x| Some(sm::clamp_amount(x as f32)))
            .ok_or_else(|| bad(c, format!("`{key}` must be a number (-1..1)"))),
    }
}

fn text<'a>(c: &str, p: &'a Value, key: &str) -> Result<Option<&'a str>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(bad(c, format!("`{key}` must be text"))),
    }
}

/// A slot index field (0..15), within the `n` slots there are.
fn slot_of(c: &str, p: &Value, n: usize) -> Result<usize> {
    let i = p
        .get("slot")
        .and_then(Value::as_u64)
        .and_then(|x| usize::try_from(x).ok())
        .ok_or_else(|| bad(c, "`slot` must be a slot number (0 first)"))?;
    if i >= n {
        return Err(bad(
            c,
            if n == 0 { "no modulation slots are in use".to_string() } else { format!("no slot {i}: slots 0..{} are in use", n - 1) },
        ));
    }
    Ok(i)
}

/// A macro field (1..4), as an index.
fn macro_of(c: &str, p: &Value) -> Result<usize> {
    p.get("macro")
        .and_then(Value::as_u64)
        .filter(|m| (1..=MAX_MACROS as u64).contains(m))
        .map(|m| m as usize - 1)
        .ok_or_else(|| bad(c, "`macro` must be 1, 2, 3 or 4"))
}

/// The source and destination named, canonical (aliases resolved), checked against the synth.
fn checked(c: &str, plugin: &str, source: &str, dest: &str) -> Result<(String, String)> {
    let (s, _) = sm::check_slot(plugin, source, dest).map_err(|m| bad(c, m))?;
    let id = sm::SOURCES.get(s).map_or(source, |s| s.id);
    Ok((id.to_string(), dest.trim().to_string()))
}

/// A whole patch (`{slots, macros}`, as a session or a preset stores it) for `plugin`, checked:
/// for a preset recall (`mix.instrument`, `mix.module_preset`). Null: none.
pub(crate) fn parse_patch(c: &str, plugin: &str, v: &Value) -> Result<SynthMod> {
    if v.is_null() {
        return Ok(SynthMod::default());
    }
    if !sm::has_matrix(plugin) {
        return Err(bad(c, format!("`{plugin}` has no modulation matrix")));
    }
    let mut m: SynthMod =
        serde_json::from_value(v.clone()).map_err(|err| bad(c, format!("`modulation` is not a patch ({{slots, macros}}): {err}")))?;
    if m.slots.len() > MAX_MOD_SLOTS || m.macros.len() > MAX_MACROS || m.macros.iter().any(|x| x.targets.len() > MAX_MACRO_TARGETS) {
        return Err(bad(
            c,
            format!("`modulation` holds more than {MAX_MOD_SLOTS} slots, {MAX_MACROS} macros or {MAX_MACRO_TARGETS} targets a macro"),
        ));
    }
    for s in &mut m.slots {
        let (source, dest) = checked(c, plugin, &s.source, s.dest.trim())?;
        s.source = source;
        s.dest = dest;
    }
    for t in m.macros.iter().flat_map(|x| x.targets.iter()) {
        if !sm::dests(plugin).contains(&t.dest.as_str()) {
            return Err(bad(c, format!("`{plugin}` cannot modulate `{}`", t.dest)));
        }
    }
    m.sanitize();
    Ok(m)
}

/// After an edit: the patch tidied and made ready for the synth.
fn done(ins: &mut Insert) {
    ins.modulation.sanitize();
    super::module_ceiling::prepare_insert(ins);
}

fn mod_add(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.mod_add";
    let source = text(C, p, "source")?.ok_or_else(|| bad(C, "`source` required"))?;
    let dest = text(C, p, "dest")?.ok_or_else(|| bad(C, "`dest` required"))?;
    let amount = amount_of(C, p, "amount")?.unwrap_or(0.25);
    let ins = synth_mut(e, C, p)?;
    let (source, dest) = checked(C, &ins.plugin, source, dest.trim())?;
    if ins.modulation.slots.len() >= MAX_MOD_SLOTS {
        return Err(bad(C, format!("all {MAX_MOD_SLOTS} modulation slots are in use: remove one first")));
    }
    ins.modulation.slots.push(ModSlot { source, dest, amount });
    let slot = ins.modulation.slots.len() - 1;
    done(ins);
    Ok(json!({"slot": slot, "slots": ins.modulation.slots.len()}))
}

fn mod_set(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.mod_set";
    let source = text(C, p, "source")?;
    let dest = text(C, p, "dest")?;
    let amount = amount_of(C, p, "amount")?;
    let ins = synth_mut(e, C, p)?;
    let i = slot_of(C, p, ins.modulation.slots.len())?;
    let plugin = ins.plugin.clone();
    let Some(slot) = ins.modulation.slots.get_mut(i) else { return Err(bad(C, "no such slot")) };
    let (source, dest) = checked(C, &plugin, source.unwrap_or(&slot.source), dest.unwrap_or(&slot.dest).trim())?;
    *slot = ModSlot { source, dest, amount: amount.unwrap_or(slot.amount) };
    done(ins);
    Ok(json!({"slot": i}))
}

fn mod_remove(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.mod_remove";
    let all = matches!(p.get("slot"), Some(Value::String(s)) if s.trim().eq_ignore_ascii_case("all"));
    let ins = synth_mut(e, C, p)?;
    if all {
        ins.modulation.slots.clear();
    } else {
        let i = slot_of(C, p, ins.modulation.slots.len())?;
        ins.modulation.slots.remove(i);
    }
    done(ins);
    Ok(json!({"slots": ins.modulation.slots.len()}))
}

/// Macro `i` of `m`, made (with any before it) if missing.
fn macro_mut(m: &mut SynthMod, i: usize) -> Option<&mut SynthMacro> {
    while m.macros.len() <= i && m.macros.len() < MAX_MACROS {
        m.macros.push(SynthMacro::default());
    }
    m.macros.get_mut(i)
}

fn macro_assign(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.macro_assign";
    let i = macro_of(C, p)?;
    let dest = text(C, p, "dest")?.ok_or_else(|| bad(C, "`dest` required"))?.trim().to_string();
    let remove = match p.get("remove") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(C, "`remove` must be true or false")),
    };
    let amount = amount_of(C, p, "amount")?;
    let from = amount_of(C, p, "from")?;
    let to = amount_of(C, p, "to")?;
    if amount.is_some() && (from.is_some() || to.is_some()) {
        return Err(bad(C, "give `amount`, or `from`/`to`, not both"));
    }
    let ins = synth_mut(e, C, p)?;
    if !sm::dests(&ins.plugin).contains(&dest.as_str()) {
        return Err(bad(C, format!("`{}` cannot modulate `{dest}` (one of: {})", ins.plugin, sm::dests(&ins.plugin).join(", "))));
    }
    let Some(mac) = macro_mut(&mut ins.modulation, i) else { return Err(bad(C, "no such macro")) };
    let at = mac.targets.iter().position(|t| t.dest == dest);
    if remove {
        let at = at.ok_or_else(|| bad(C, format!("Macro {} does not drive `{dest}`", i + 1)))?;
        mac.targets.remove(at);
    } else {
        let old = at.and_then(|a| mac.targets.get(a)).cloned();
        let (f, t) = match (amount, from, to) {
            (Some(a), _, _) => (0.0, a),
            (None, None, None) => old.as_ref().map_or((0.0, 0.5), |o| (o.from, o.to)),
            (None, f, t) => (f.or(old.as_ref().map(|o| o.from)).unwrap_or(0.0), t.or(old.as_ref().map(|o| o.to)).unwrap_or(0.0)),
        };
        let target = MacroTarget { dest: dest.clone(), from: f, to: t };
        let full = mac.targets.len() >= MAX_MACRO_TARGETS;
        match at.and_then(|a| mac.targets.get_mut(a)) {
            Some(slot) => *slot = target,
            None if full => {
                return Err(bad(C, format!("Macro {} already drives {MAX_MACRO_TARGETS} destinations: remove one first", i + 1)));
            }
            None => mac.targets.push(target),
        }
    }
    let targets = json!(mac.targets);
    done(ins);
    Ok(json!({"macro": i + 1, "targets": targets}))
}

fn macro_name(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.macro_name";
    let i = macro_of(C, p)?;
    let name: String =
        text(C, p, "name")?.ok_or_else(|| bad(C, "`name` required (empty for the default)"))?.trim().chars().take(MAX_MACRO_NAME).collect();
    if name.chars().any(char::is_control) {
        return Err(bad(C, "`name` must not hold control characters"));
    }
    let ins = synth_mut(e, C, p)?;
    if let Some(mac) = macro_mut(&mut ins.modulation, i) {
        mac.name = name;
    }
    done(ins);
    Ok(json!({"macro": i + 1, "name": ins.modulation.macro_name(i)}))
}

/// What Surprise may patch, with the largest amount each way it may use: musical, never loud.
/// Pitch within a fifth (7 of the 48 semitones' travel), the level at most +3 dB.
fn surprise_limit(plugin: &str, dest: &str) -> Option<f32> {
    let info = sm::dest_info(plugin, dest)?;
    let span = info.max - info.min;
    Some(match dest {
        sm::PITCH | "osc_pitch" | "osc2_semitones" => 7.0 / span,
        "level" => 3.0 / span,
        "osc_fine" | "osc2_detune" => 0.15,
        _ if dest.ends_with("_freq") => return None,
        sm::SHARED_DEST => 0.15,
        _ => 0.4,
    })
}

/// A small deterministic generator for Surprise (xorshift64*).
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        let mut x = self.0.max(1);
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }

    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

fn mod_surprise(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.mod_surprise";
    let seed = match p.get("seed") {
        None | Some(Value::Null) => std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64),
        Some(v) => v.as_u64().or_else(|| v.as_i64().map(|x| x as u64)).ok_or_else(|| bad(C, "`seed` must be a whole number"))?,
    };
    let ins = synth_mut(e, C, p)?;
    let room = MAX_MOD_SLOTS.saturating_sub(ins.modulation.slots.len());
    if room == 0 {
        return Err(bad(C, format!("all {MAX_MOD_SLOTS} modulation slots are in use: remove some first")));
    }
    let mut dice = Dice(seed ^ 0x9E37_79B9_7F4A_7C15);
    let want = (2 + dice.below(2)).min(room);
    let dests: Vec<&str> = sm::dests(&ins.plugin).iter().copied().filter(|d| surprise_limit(&ins.plugin, d).is_some()).collect();
    let mut added = Vec::new();
    let mut tries = 0;
    while added.len() < want && tries < 64 {
        tries += 1;
        let Some(&dest) = dests.get(dice.below(dests.len())) else { break };
        let sources: Vec<&sm::ModSource> = sm::SOURCES.iter().filter(|s| !(s.per_voice && !sm::dest_per_voice(dest))).collect();
        let Some(src) = sources.get(dice.below(sources.len())) else { continue };
        // One cable per destination and source.
        if ins.modulation.slots.iter().any(|s| s.dest == dest && s.source == src.id) {
            continue;
        }
        let limit = surprise_limit(&ins.plugin, dest).unwrap_or(0.0);
        // At least a third of the limit, so it is heard; either way.
        let mag = limit * (0.35 + 0.65 * dice.unit());
        let amount = if dice.below(2) == 0 { mag } else { -mag };
        ins.modulation.slots.push(ModSlot { source: src.id.to_string(), dest: dest.to_string(), amount: (amount * 1000.0).round() / 1000.0 });
        added.push(ins.modulation.slots.len() - 1);
    }
    done(ins);
    Ok(json!({"added": added, "slots": ins.modulation.slots.len()}))
}

/// The range a destination's knob is moved over: per slot the offsets its source can reach,
/// summed; plus the macros' offsets at their values now. In knob travel (0..1 of its sweep).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ModArc {
    pub dest: String,
    pub lo: f32,
    pub hi: f32,
    pub cables: Vec<ArcCable>,
    pub macro_offset: f32,
}

/// One slot's share of a [`ModArc`].
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ArcCable {
    pub slot: usize,
    pub source: String,
    pub lo: f32,
    pub hi: f32,
}

/// The arcs of every destination `ins` moves (none for a module without the matrix), in the
/// order of the synth's destination list.
pub fn arcs(ins: &Insert) -> Vec<ModArc> {
    let mut out = Vec::new();
    for dest in sm::dests(&ins.plugin) {
        let mut cables = Vec::new();
        for (slot, s) in ins.modulation.slots.iter().enumerate().filter(|(_, s)| s.dest == *dest) {
            let Some(src) = sm::source_index(&s.source).and_then(|i| sm::SOURCES.get(i)) else { continue };
            let (a, b) = (src.lo * s.amount, src.hi * s.amount);
            cables.push(ArcCable { slot, source: src.id.to_string(), lo: a.min(b), hi: a.max(b) });
        }
        let mut macro_offset = 0.0;
        let mut driven = false;
        for (i, mac) in ins.modulation.macros.iter().enumerate() {
            let v = sm::MACRO_PARAMS.get(i).and_then(|p| ins.params.get(*p)).copied().unwrap_or(0.0) / 100.0;
            for t in mac.targets.iter().filter(|t| t.dest == *dest) {
                macro_offset += t.from + (t.to - t.from) * v;
                driven = true;
            }
        }
        if cables.is_empty() && !driven {
            continue;
        }
        let lo = cables.iter().map(|c| c.lo).sum::<f32>() + macro_offset;
        let hi = cables.iter().map(|c| c.hi).sum::<f32>() + macro_offset;
        out.push(ModArc { dest: dest.to_string(), lo, hi, cables, macro_offset });
    }
    out
}

fn mod_info(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.mod_info";
    let ins = synth_ref(e, C, p)?;
    let plugin = ins.plugin.as_str();
    let dests: Vec<Value> = sm::dests(plugin)
        .iter()
        .filter_map(|d| sm::dest_info(plugin, d).map(|i| json!({"id": d, "name": i.name, "per_voice": sm::dest_per_voice(d)})))
        .collect();
    let slots: Vec<Value> =
        ins.modulation.slots.iter().enumerate().map(|(i, s)| json!({"slot": i, "source": s.source, "dest": s.dest, "amount": s.amount})).collect();
    let macros: Vec<Value> = (0..MAX_MACROS)
        .map(|i| {
            let param = sm::MACRO_PARAMS.get(i).copied().unwrap_or("");
            json!({
                "macro": i + 1,
                "param": param,
                "name": ins.modulation.macro_name(i),
                "value": ins.params.get(param).copied().unwrap_or(0.0) / 100.0,
                "targets": ins.modulation.macros.get(i).map(|m| json!(m.targets)).unwrap_or_else(|| json!([])),
            })
        })
        .collect();
    Ok(json!({
        "plugin": plugin,
        "sources": sm::SOURCES,
        "dests": dests,
        "slots": slots,
        "macros": macros,
        "arcs": arcs(ins),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use soundcraft_model::TrackId;

    fn synth_track(plugin: &str) -> (Engine, u64) {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"name": "K", "kind": "instrument"})).unwrap();
        let t = e.session().tracks[0].id;
        e.execute("mix.instrument", &json!({"track": t.0, "plugin": plugin})).unwrap();
        (e, t.0)
    }

    fn inst(e: &Engine, t: u64) -> &Insert {
        e.session().track(TrackId(t)).unwrap().instrument.as_ref().unwrap()
    }

    #[test]
    fn a_patched_synth_is_made_ready_and_an_empty_one_is_not() {
        let mut ins = Insert::new("subtractive_synth");
        super::super::module_ceiling::prepare_insert(&mut ins);
        assert!(ins.prepared.is_none());
        ins.modulation = SynthMod { slots: vec![ModSlot { source: "lfo".into(), dest: "cutoff".into(), amount: 0.2 }], macros: vec![] };
        super::super::module_ceiling::prepare_insert(&mut ins);
        assert!(ins.prepared.as_ref().is_some_and(|p| p.data.len() > 3));
    }

    #[test]
    fn slots_add_set_and_remove_one_undo_step_each() {
        let (mut e, t) = synth_track("subtractive_synth");
        let undo = e.undo_history().len();
        let r = e.execute("mix.mod_add", &json!({"track": t, "source": "rnd", "dest": "cutoff", "amount": 3})).unwrap();
        assert_eq!(r["slot"], 0);
        assert_eq!(inst(&e, t).modulation.slots[0], ModSlot { source: "random".into(), dest: "cutoff".into(), amount: 1.0 });
        assert!(inst(&e, t).prepared.is_some(), "made ready for the synth");
        e.execute("mix.mod_set", &json!({"track": t, "slot": 0, "amount": -0.5, "source": "lfo"})).unwrap();
        assert_eq!(inst(&e, t).modulation.slots[0].amount, -0.5);
        assert_eq!(e.undo_history().len(), undo + 2);
        assert_eq!(e.undo_label(), Some("Set Modulation"));
        e.execute("mix.mod_remove", &json!({"track": t, "slot": 0})).unwrap();
        assert!(inst(&e, t).modulation.is_empty());
        assert!(inst(&e, t).prepared.is_none(), "nothing patched: the synth as before");
        e.undo();
        assert_eq!(inst(&e, t).modulation.slots.len(), 1);
        assert!(inst(&e, t).prepared.is_some(), "undo brings back the prepared patch too");
    }

    #[test]
    fn hostile_and_wrong_input_are_clear_errors() {
        let (mut e, t) = synth_track("fm_synth");
        for bad_params in [
            json!({"track": t, "source": "lfo", "dest": "cutoff"}),
            json!({"track": t, "source": "nope", "dest": "pitch"}),
            json!({"track": t, "source": "velocity", "dest": "lfo_rate"}),
            json!({"track": t, "source": "lfo", "dest": "pitch", "amount": "x"}),
            json!({"track": t, "source": 7, "dest": "pitch"}),
            json!({"track": t, "dest": "pitch"}),
            json!({"track": t, "module": "drums", "source": "lfo", "dest": "pitch"}),
            json!({"track": t, "module": 99, "source": "lfo", "dest": "pitch"}),
        ] {
            assert!(e.execute("mix.mod_add", &bad_params).is_err(), "{bad_params}");
        }
        let err = e.execute("mix.mod_add", &json!({"track": t, "source": "lfo", "dest": "cutoff"})).unwrap_err().to_string();
        assert!(err.contains("op1_level"), "{err}");
        assert!(e.execute("mix.mod_set", &json!({"track": t, "slot": 0, "amount": 0.1})).is_err());
        assert!(e.execute("mix.mod_remove", &json!({"track": t, "slot": u64::MAX})).is_err());
        assert!(e.execute("mix.macro_assign", &json!({"track": t, "macro": 0, "dest": "pitch"})).is_err());
        assert!(e.execute("mix.macro_assign", &json!({"track": t, "macro": 5, "dest": "pitch"})).is_err());
        assert!(e.execute("mix.macro_assign", &json!({"track": t, "macro": 1, "dest": "pitch", "amount": 1, "to": 1})).is_err());
        assert!(e.execute("mix.macro_name", &json!({"track": t, "macro": 1, "name": "a\u{7}b"})).is_err());
        for _ in 0..16 {
            e.execute("mix.mod_add", &json!({"track": t, "source": "lfo", "dest": "pitch", "amount": f64::MAX})).unwrap();
        }
        let full = e.execute("mix.mod_add", &json!({"track": t, "source": "lfo", "dest": "pitch"})).unwrap_err().to_string();
        assert!(full.contains("16"), "{full}");
        assert!(e.execute("mix.mod_surprise", &json!({"track": t, "seed": 1})).is_err());
        // A plugin without the matrix.
        let (mut d, dt) = synth_track("drum_synth");
        assert!(
            d.execute("mix.mod_add", &json!({"track": dt, "source": "lfo", "dest": "pitch"}))
                .unwrap_err()
                .to_string()
                .contains("no modulation matrix")
        );
    }

    #[test]
    fn macros_are_named_and_assigned_and_show_in_the_arcs() {
        let (mut e, t) = synth_track("wavetable_synth");
        e.execute("mix.macro_name", &json!({"track": t, "macro": 2, "name": "  Shine and shimmer forever and ever  "})).unwrap();
        assert_eq!(inst(&e, t).modulation.macro_name(1).chars().count(), MAX_MACRO_NAME);
        assert_eq!(inst(&e, t).modulation.macro_name(0), "Macro 1");
        e.execute("mix.macro_assign", &json!({"track": t, "macro": 2, "dest": "cutoff", "amount": 0.4})).unwrap();
        e.execute("mix.macro_assign", &json!({"track": t, "macro": 2, "dest": "cutoff", "from": -0.2})).unwrap();
        let m = &inst(&e, t).modulation.macros[1];
        assert_eq!((m.targets.len(), m.targets[0].from, m.targets[0].to), (1, -0.2, 0.4));
        e.execute("mix.instrument_param", &json!({"track": t, "param": "macro2", "value": 50})).unwrap();
        e.execute("mix.mod_add", &json!({"track": t, "source": "lfo", "dest": "cutoff", "amount": 0.25})).unwrap();
        let info = e.execute("mix.mod_info", &json!({"track": t})).unwrap();
        let arc = &info["arcs"][0];
        assert_eq!(arc["dest"], "cutoff");
        let close = |v: &Value, x: f64| (v.as_f64().unwrap() - x).abs() < 1e-5;
        assert!(close(&arc["macro_offset"], 0.1) && close(&arc["lo"], -0.15) && close(&arc["hi"], 0.35), "{arc}");
        assert_eq!(info["macros"][1]["name"].as_str().unwrap().chars().count(), MAX_MACRO_NAME);
        assert_eq!(info["sources"].as_array().unwrap().len(), 7);
        e.execute("mix.macro_assign", &json!({"track": t, "macro": 2, "dest": "cutoff", "remove": true})).unwrap();
        assert!(inst(&e, t).modulation.macros[1].targets.is_empty());
    }

    /// A preset recall carries the patch (`mix.instrument`, `mix.module_preset`), replaces it,
    /// and refuses a patch that names what the synth does not have; an old session's synth (no
    /// `modulation`) opens with none; a saved one round-trips.
    #[test]
    fn presets_and_sessions_carry_the_patch() {
        let (mut e, t) = synth_track("subtractive_synth");
        let patch = json!({"slots": [{"source": "vel", "dest": "cutoff", "amount": 0.3}], "macros": [{"name": "Open", "targets": [{"dest": "cutoff", "from": 0, "to": 0.4}]}]});
        e.execute("mix.instrument", &json!({"track": t, "plugin": "subtractive_synth", "params": {"cutoff": 900}, "modulation": patch})).unwrap();
        let m = &inst(&e, t).modulation;
        assert_eq!((m.slots[0].source.as_str(), m.macro_name(0).as_str()), ("velocity", "Open"));
        assert!(inst(&e, t).prepared.is_some());
        let text = serde_json::to_string(e.session()).unwrap();
        let back: soundcraft_model::Session = serde_json::from_str(&text).unwrap();
        assert_eq!(back.track(TrackId(t)).unwrap().instrument.as_ref().unwrap().modulation, *m);
        // A preset without a patch clears it.
        e.execute("mix.module_preset", &json!({"track": t, "module": "instrument", "preset": "Plain", "params": {"cutoff": 2000}})).unwrap();
        assert!(inst(&e, t).modulation.is_empty() && inst(&e, t).prepared.is_none());
        e.execute("mix.module_preset", &json!({"track": t, "module": "instrument", "preset": "P", "params": {}, "modulation": patch})).unwrap();
        assert_eq!(inst(&e, t).modulation.slots.len(), 1);
        for bad_patch in [json!({"slots": [{"source": "lfo", "dest": "op1_level"}]}), json!({"slots": "x"}), json!([1, 2])] {
            assert!(
                e.execute("mix.instrument", &json!({"track": t, "plugin": "subtractive_synth", "modulation": bad_patch})).is_err(),
                "{bad_patch}"
            );
        }
        assert!(e.execute("mix.instrument", &json!({"track": t, "plugin": "drum_synth", "modulation": patch})).is_err());
        let old: Insert = serde_json::from_str(r#"{"plugin":"fm_synth","params":{"op1_level":80}}"#).unwrap();
        assert!(old.modulation.is_empty());
    }

    #[test]
    fn surprise_is_deterministic_and_stays_in_safe_ranges() {
        for plugin in ["subtractive_synth", "wavetable_synth", "fm_synth"] {
            let run = |seed: u64| {
                let (mut e, t) = synth_track(plugin);
                let r = e.execute("mix.mod_surprise", &json!({"track": t, "seed": seed})).unwrap();
                let n = r["added"].as_array().unwrap().len();
                assert!((2..=3).contains(&n), "{plugin}: {r}");
                inst(&e, t).modulation.clone()
            };
            assert_eq!(run(7), run(7), "{plugin}");
            for seed in 0..200 {
                for s in run(seed).slots {
                    let info = sm::dest_info(plugin, &s.dest).unwrap();
                    let span = info.max - info.min;
                    match s.dest.as_str() {
                        "pitch" | "osc_pitch" | "osc2_semitones" => assert!(s.amount.abs() * span <= 7.0 + 1e-3, "{plugin}: {s:?}"),
                        "level" => assert!(s.amount.abs() * span <= 3.0 + 1e-3, "{plugin}: {s:?}"),
                        _ => assert!(s.amount.abs() <= 0.4 + 1e-6, "{plugin}: {s:?}"),
                    }
                    assert!(sm::check_slot(plugin, &s.source, &s.dest).is_ok());
                }
            }
        }
    }
}
