//! Channel-chain module commands (`mix.module_*`). Modules are addressed by their stable id;
//! the `mix.insert_*` commands are aliases that resolve a chain position to the id and call the
//! same functions.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_dsp::{Derive, ParamEdit};
use soundcraft_model::{AutoParam, ChainError, Insert, MAX_MODULES, ModuleId, SidechainSource, StripSize, Track, b64};
use std::collections::BTreeMap;

/// Most knobs a module can pin to its strip face.
const MAX_KNOBS: usize = 4;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.module_add",
            "Add Module",
            [],
            None,
            "{track?, plugin: id, at?: position in the chain (absent or past the end appends), params?: {id: value}} → {module, index}",
            has_selection,
            module_add
        ),
        cmd!("mix.module_remove", "Remove Module", [], None, "{track?, module: id} → {index}", has_selection, module_remove),
        cmd!(
            "mix.module_move",
            "Move Module",
            [],
            None,
            "{track?, module: id, to: position in the chain, to_track?} → {index}; with another to_track the module moves there with its automation lanes (a new id) → {module, index}",
            has_selection,
            module_move
        ),
        cmd!("mix.module_bypass", "Bypass Module", [], None, "{track?, module: id, value?: bool} → {bypass}", has_selection, module_bypass),
        cmd!(
            "mix.module_param",
            "Set Module Parameter",
            [],
            None,
            "{track?, module: id, param, value} → {value} (clamped; a plugin's derived parameters follow, e.g. Dist's Link sets Level)",
            has_selection,
            module_param
        ),
        cmd!(
            "mix.module_params",
            "Set Module Parameters",
            [],
            None,
            "{track?, module: id, params: {id: value}, preset?: name} → {set} (unknown ids and non-numeric values are errors; with `preset` the values are a preset recall for the plugin's derived parameters)",
            has_selection,
            module_params
        ),
        CommandSpec {
            id: "mix.module_state",
            label: "Store Module State",
            menu: &[],
            shortcut: None,
            params: "{track, module: id | \"instrument\", state: base64 | null} → {bytes}. Not undoable and not journaled (states can be large).",
            enabled: has_tracks,
            run: module_state,
            journal: false,
            undoable: false,
            view: false,
        },
        cmd!(
            "mix.module_state_edit",
            "Edit Module",
            [],
            None,
            "{track?, module: id, state: base64 | null} → {bytes}. A built-in module's own editor changing its stored state (CleanQ's Ceiling panel): undoable, at most 64 KiB (`mix.module_state` stores third-party plugin blobs and is not undoable).",
            has_selection,
            module_state_edit
        ),
        cmd!(
            "mix.module_copy",
            "Copy Module",
            [],
            None,
            "{track?, module: id, to_track, at?: position in the chain} → {module, index} (a new id on to_track; automation is not copied)",
            has_selection,
            module_copy
        ),
        cmd!(
            "mix.module_knobs",
            "Set Module Knobs",
            [],
            None,
            "{track?, module: id, knobs: [parameter id] (at most 4)} → {knobs}",
            has_selection,
            module_knobs
        ),
        cmd!(
            "mix.module_sidechain",
            "Set Module Sidechain",
            [],
            None,
            "{track?, module: id, source: track id or name | null, pre_fader?: bool (default false: after the source's fader and mute)} → {sidechain: {track, pre_fader} | null}. The module's plugin must take a sidechain. Refused: the module's own track, a master or VCA track, an aux or folder keying an audio/instrument/MIDI track, and anything that would form a routing cycle through busses or other sidechains. A muted, inactive or later deleted source gives a silent key.",
            has_selection,
            module_sidechain
        ),
        cmd!(
            "mix.strip_size",
            "Strip Size",
            [],
            None,
            "{track? | tracks?: [ids or names] | all?: bool, size: \"auto\" | \"wide\" | \"slim\"} → {size, tracks: n} (auto follows the Mix window's global wide/slim toggle). Without `track`/`tracks`/`all` it sets the selected tracks; several tracks are one undo step.",
            always,
            strip_size
        ),
    ]
}

// ---- shared cores (also used by the `mix.insert_*` aliases) -------------------------------------

pub(super) fn first_track(e: &Engine, cmd: &str, p: &Value) -> Result<TrackId> {
    tracks_required(e, cmd, p)?.first().copied().ok_or_else(|| bad(cmd, "no track"))
}

pub(super) fn module_id(cmd: &str, p: &Value) -> Result<ModuleId> {
    p.get("module").and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).map(ModuleId).ok_or_else(|| bad(cmd, "`module` must be a module id"))
}

fn index_param(cmd: &str, p: &Value, key: &str) -> Result<Option<usize>> {
    match p.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            v.as_u64().and_then(|n| usize::try_from(n).ok()).map(Some).ok_or_else(|| bad(cmd, format!("`{key}` must be a position in the chain")))
        }
    }
}

fn track_of<'a>(e: &'a mut Engine, cmd: &str, t: TrackId) -> Result<&'a mut Track> {
    e.session_mut().track_mut(t).ok_or_else(|| bad(cmd, "no track"))
}

/// The module `id` on track `t`.
pub(super) fn find_mut<'a>(e: &'a mut Engine, cmd: &str, t: TrackId, id: ModuleId) -> Result<&'a mut Insert> {
    let tr = track_of(e, cmd, t)?;
    let name = tr.name.clone();
    tr.mixer.module_mut(id).ok_or_else(|| bad(cmd, format!("no module {} on {name}", id.0)))
}

/// A fresh module of `plugin` with every parameter at its default, then `params` applied
/// (clamped; unknown ids are errors, a non-numeric value keeps the default).
pub(super) fn build_insert(cmd: &str, plugin: &str, params: Option<&Value>) -> Result<Insert> {
    let info = super::clap::plugin_info(plugin).ok_or_else(|| bad(cmd, format!("unknown plugin `{plugin}`")))?;
    let mut ins = Insert::new(info.id);
    for pi in info.params {
        ins.params.insert(pi.id.to_string(), pi.default);
    }
    if let Some(obj) = params.and_then(Value::as_object) {
        for (k, v) in obj {
            let Some(pi) = info.params.iter().find(|x| x.id == k) else {
                return Err(bad(cmd, format!("`{plugin}` has no parameter `{k}`")));
            };
            let val = v.as_f64().filter(|f| f.is_finite()).map_or(pi.default, |f| (f as f32).clamp(pi.min, pi.max));
            ins.params.insert(k.clone(), val);
        }
        // The initial values are an edit of a fresh instance: its derived parameters follow (a
        // fresh module has no automation lanes yet).
        if let (Some(derive), Some(dinfo)) = (soundcraft_dsp::param_deriver(info.id), soundcraft_dsp::plugin_info(info.id)) {
            let before: BTreeMap<String, f32> = info.params.iter().map(|p| (p.id.to_string(), p.default)).collect();
            let changed: Vec<String> = obj.keys().cloned().collect();
            let mut d = Derive::new(dinfo, &before, &mut ins.params, &mut ins.memory, &changed, ParamEdit::Edit, &|_| false);
            derive(&mut d);
        }
    }
    Ok(ins)
}

/// Adds `ins` at `at` (clamped; `None` = the end) and returns its new id and position.
pub(super) fn chain_add(tr: &mut Track, cmd: &str, ins: Insert, at: Option<usize>) -> Result<(ModuleId, usize)> {
    let id = tr.mixer.add_module(ins, at).map_err(|err| match err {
        ChainError::Full => bad(cmd, format!("chain is full ({MAX_MODULES} modules)")),
        ChainError::OutOfIds => bad(cmd, err.to_string()),
    })?;
    Ok((id, tr.mixer.module_index(id).unwrap_or_default()))
}

/// Removes the module and its `AutoParam::Plugin` lanes; returns its former position.
pub(super) fn chain_remove(tr: &mut Track, id: ModuleId) -> Option<usize> {
    let (at, _) = tr.mixer.remove_module(id)?;
    let gone = |p: &AutoParam| matches!(p, AutoParam::Plugin { module, .. } if *module == id);
    if tr.automation.iter().any(|l| gone(&l.param) && l.param.id() == tr.view) {
        tr.view = if tr.kind.is_midi() {
            "notes"
        } else if tr.kind.has_playlist() {
            "waveform"
        } else {
            "volume"
        }
        .into();
    }
    tr.automation.retain(|l| !gone(&l.param));
    tr.auto_lanes.retain(|v| AutoParam::parse(v).is_none_or(|p| !gone(&p)));
    Some(at)
}

pub(super) fn do_bypass(e: &mut Engine, cmd: &str, t: TrackId, id: ModuleId, value: Option<bool>) -> Result<Value> {
    let ins = find_mut(e, cmd, t, id)?;
    ins.bypass = value.unwrap_or(!ins.bypass);
    Ok(json!({"bypass": ins.bypass}))
}

pub(super) fn do_param(e: &mut Engine, cmd: &str, t: TrackId, id: ModuleId, p: &Value) -> Result<Value> {
    let param = str_param(p, "param").ok_or_else(|| bad(cmd, "`param` required"))?.to_string();
    let value = p.get("value").and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| bad(cmd, "`value` must be a number"))? as f32;
    let ins = find_mut(e, cmd, t, id)?;
    let info = super::clap::plugin_info(&ins.plugin).ok_or_else(|| bad(cmd, "unknown plugin"))?;
    let pi = info.params.iter().find(|x| x.id == param).ok_or_else(|| bad(cmd, format!("`{}` has no parameter `{param}`", info.id)))?;
    let v = value.clamp(pi.min, pi.max);
    let before = ins.params.clone();
    ins.params.insert(param.clone(), v);
    derive_params(e, t, id, &before, std::slice::from_ref(&param), ParamEdit::Edit);
    e.automation_changed(t, AutoParam::Plugin { module: id, param });
    Ok(json!({"value": v}))
}

/// Runs the plugin's derived-parameter logic, if it has any (`soundcraft_dsp::derive`: a C++
/// wrapper's own parameter listeners, such as FireStation Dist's Link), after an edit of
/// `changed`. Part of the same command, so one undo step takes back the edit and what it
/// derived. A parameter with an automation lane is left to its lane.
pub(super) fn derive_params(e: &mut Engine, t: TrackId, id: ModuleId, before: &BTreeMap<String, f32>, changed: &[String], edit: ParamEdit) {
    let Some(tr) = e.session_mut().track_mut(t) else { return };
    // As the mix engine: a lane drives its parameter only while the track reads automation.
    let reads = tr.mixer.automation_mode.reads();
    let laned: Vec<String> = tr
        .automation
        .iter()
        .filter(|l| reads && !l.points.is_empty())
        .filter_map(|l| match &l.param {
            AutoParam::Plugin { module, param } if *module == id => Some(param.clone()),
            _ => None,
        })
        .collect();
    let Some(ins) = tr.mixer.module_mut(id) else { return };
    let (Some(derive), Some(info)) = (soundcraft_dsp::param_deriver(&ins.plugin), soundcraft_dsp::plugin_info(&ins.plugin)) else { return };
    let locked = |p: &str| laned.iter().any(|l| l == p);
    let mut d = Derive::new(info, before, &mut ins.params, &mut ins.memory, changed, edit, &locked);
    derive(&mut d);
}

/// The most keys a `mix.module_params {memory}` may carry (Dist's bank holds 36).
const MAX_MEMORY_KEYS: usize = 256;

/// Apply many parameters at once (preset recall); values are clamped. Unknown ids and
/// non-numeric values are errors, not silent no-ops: a typo must not look like success.
pub(super) fn do_params(e: &mut Engine, cmd: &str, t: TrackId, id: ModuleId, p: &Value) -> Result<Value> {
    let obj = p.get("params").and_then(Value::as_object).cloned().ok_or_else(|| bad(cmd, "`params` must be an object"))?;
    let preset = str_param(p, "preset").map(str::to_string);
    let ins = find_mut(e, cmd, t, id)?;
    let info = super::clap::plugin_info(&ins.plugin).ok_or_else(|| bad(cmd, "unknown plugin"))?;
    let mut unknown: Vec<&str> = Vec::new();
    let mut non_numeric: Vec<&str> = Vec::new();
    for (k, v) in &obj {
        if !info.params.iter().any(|x| x.id == k.as_str()) {
            unknown.push(k.as_str());
        } else if v.as_f64().is_none_or(|f| !f.is_finite()) {
            non_numeric.push(k.as_str());
        }
    }
    if !unknown.is_empty() {
        unknown.sort_unstable();
        return Err(bad(cmd, format!("`{}` has no parameter(s): {}", info.id, unknown.join(", "))));
    }
    if !non_numeric.is_empty() {
        non_numeric.sort_unstable();
        return Err(bad(cmd, format!("parameter(s) need a numeric value: {}", non_numeric.join(", "))));
    }
    // `memory`: the module's derived-parameter memory restored with the values (an editor's
    // A/B or undo snapshot, as DistEngine's `restore` puts its knob bank back first). The values
    // then land as a recall, not an edit: no knob swap from the old bank.
    let memory = match p.get("memory") {
        None | Some(Value::Null) => None,
        Some(Value::Object(_)) if soundcraft_dsp::param_deriver(&ins.plugin).is_none() => {
            return Err(bad(cmd, format!("`{}` has no derived-parameter memory", info.id)));
        }
        Some(Value::Object(m)) => {
            if m.len() > MAX_MEMORY_KEYS || m.iter().any(|(k, v)| k.len() > 64 || v.as_f64().is_none_or(|f| !f.is_finite())) {
                return Err(bad(cmd, format!("`memory` must map at most {MAX_MEMORY_KEYS} short keys to finite numbers")));
            }
            Some(m.iter().filter_map(|(k, v)| v.as_f64().map(|f| (k.clone(), f as f32))).collect::<BTreeMap<String, f32>>())
        }
        Some(_) => return Err(bad(cmd, "`memory` must be an object")),
    };
    let restoring = memory.is_some();
    if let Some(m) = memory {
        ins.memory = m;
    }
    let before = ins.params.clone();
    let mut changed = Vec::new();
    for (k, v) in obj {
        if let (Some(pi), Some(f)) = (info.params.iter().find(|x| x.id == k), v.as_f64().filter(|f| f.is_finite())) {
            ins.params.insert(k.clone(), (f as f32).clamp(pi.min, pi.max));
            changed.push(k);
        }
    }
    let n = changed.len();
    let edit = if preset.is_some() || restoring { ParamEdit::Preset } else { ParamEdit::Edit };
    if let Some(name) = preset {
        ins.preset = name;
    }
    derive_params(e, t, id, &before, &changed, edit);
    Ok(json!({"set": n}))
}

pub(super) fn do_remove(e: &mut Engine, cmd: &str, t: TrackId, id: ModuleId) -> Result<Value> {
    let tr = track_of(e, cmd, t)?;
    let name = tr.name.clone();
    let index = chain_remove(tr, id).ok_or_else(|| bad(cmd, format!("no module {} on {name}", id.0)))?;
    Ok(json!({"index": index}))
}

pub(super) fn do_move(e: &mut Engine, cmd: &str, t: TrackId, id: ModuleId, to: usize) -> Result<Value> {
    let tr = track_of(e, cmd, t)?;
    let name = tr.name.clone();
    if !tr.mixer.move_module(id, to) {
        return Err(bad(cmd, format!("no module {} on {name}", id.0)));
    }
    Ok(json!({"index": tr.mixer.module_index(id).unwrap_or_default()}))
}

/// Moves module `id` of track `t` to position `to` of track `dest` (a new id there), with its
/// automation lanes re-keyed to that id.
fn do_move_across(e: &mut Engine, cmd: &str, t: TrackId, id: ModuleId, dest: TrackId, to: usize) -> Result<Value> {
    let src = track_of(e, cmd, t)?;
    let ins = src.mixer.module(id).cloned().ok_or_else(|| bad(cmd, format!("no module {} on {}", id.0, src.name)))?;
    let of_module = |p: &AutoParam| matches!(p, AutoParam::Plugin { module, .. } if *module == id);
    let lanes: Vec<_> = src.automation.iter().filter(|l| of_module(&l.param)).cloned().collect();
    let dst = track_of(e, cmd, dest)?;
    let (module, index) = chain_add(dst, cmd, ins, Some(to))?;
    for mut lane in lanes {
        if let AutoParam::Plugin { module: m, .. } = &mut lane.param {
            *m = module;
        }
        dst.automation.retain(|l| l.param != lane.param);
        dst.automation.push(lane);
    }
    chain_remove(track_of(e, cmd, t)?, id);
    let cleared = drop_unrenderable_sidechain(e, dest, module);
    Ok(json!({"module": module, "index": index, "sidechain_cleared": cleared}))
}

/// Stores (or, for null, clears) the base64 `state` of `p` on `ins`.
pub(super) fn store_state(cmd: &str, ins: &mut Insert, p: &Value) -> Result<Value> {
    let state = match p.get("state") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(b64::decode(s).ok_or_else(|| bad(cmd, "`state` is not valid base64 (or is too large)"))?),
        Some(_) => return Err(bad(cmd, "`state` must be a base64 string or null")),
    };
    let n = state.as_ref().map_or(0, Vec::len);
    match state {
        Some(bytes) => ins.set_state_bytes(&bytes),
        None => ins.state = None,
    }
    // A built-in's state (CleanQ's ceiling) is made ready for the audio thread here, off it.
    super::module_ceiling::prepare_insert(ins);
    Ok(json!({"bytes": n}))
}

// ---- commands ----------------------------------------------------------------------------------

fn module_add(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.module_add";
    let plugin = str_param(p, "plugin").ok_or_else(|| bad(C, "`plugin` required"))?;
    let t = first_track(e, C, p)?;
    let at = index_param(C, p, "at")?;
    let ins = build_insert(C, plugin, p.get("params"))?;
    let (module, index) = chain_add(track_of(e, C, t)?, C, ins, at)?;
    Ok(json!({"module": module, "index": index}))
}

fn module_remove(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.module_remove";
    let t = first_track(e, C, p)?;
    do_remove(e, C, t, module_id(C, p)?)
}

fn module_move(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.module_move";
    let t = first_track(e, C, p)?;
    let to = index_param(C, p, "to")?.ok_or_else(|| bad(C, "`to` required"))?;
    match track_param(e, C, p, "to_track")? {
        Some(dest) if dest != t => do_move_across(e, C, t, module_id(C, p)?, dest, to),
        _ => do_move(e, C, t, module_id(C, p)?, to),
    }
}

fn module_bypass(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.module_bypass";
    let t = first_track(e, C, p)?;
    do_bypass(e, C, t, module_id(C, p)?, p.get("value").and_then(Value::as_bool))
}

fn module_param(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.module_param";
    let t = first_track(e, C, p)?;
    do_param(e, C, t, module_id(C, p)?, p)
}

fn module_params(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.module_params";
    let t = first_track(e, C, p)?;
    do_params(e, C, t, module_id(C, p)?, p)
}

fn module_state(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.module_state";
    let t = track_param(e, C, p, "track")?.ok_or_else(|| bad(C, "`track` required"))?;
    if p.get("module").and_then(Value::as_str).is_some_and(|s| s.trim().eq_ignore_ascii_case("instrument")) {
        let ins = track_of(e, C, t)?.instrument.as_mut().ok_or_else(|| bad(C, "no instrument"))?;
        return store_state(C, ins, p);
    }
    let id = module_id(C, p)?;
    store_state(C, find_mut(e, C, t, id)?, p)
}

/// The largest state `mix.module_state_edit` takes (a CleanQ state with 24 guides is ~5 KiB).
pub const MAX_EDIT_STATE_BYTES: usize = 64 * 1024;

fn module_state_edit(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.module_state_edit";
    let t = first_track(e, C, p)?;
    let id = module_id(C, p)?;
    if let Some(Value::String(s)) = p.get("state")
        && s.len() > MAX_EDIT_STATE_BYTES.div_ceil(3) * 4 + 8
    {
        return Err(bad(C, format!("`state` is over {} KiB", MAX_EDIT_STATE_BYTES / 1024)));
    }
    store_state(C, find_mut(e, C, t, id)?, p)
}

fn module_copy(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.module_copy";
    let t = first_track(e, C, p)?;
    let to = track_param(e, C, p, "to_track")?.ok_or_else(|| bad(C, "`to_track` required"))?;
    let at = index_param(C, p, "at")?;
    let id = module_id(C, p)?;
    // Plugin id, params, bypass, preset, state and knobs all ride along in the clone.
    let ins = find_mut(e, C, t, id)?.clone();
    let (module, index) = chain_add(track_of(e, C, to)?, C, ins, at)?;
    let cleared = drop_unrenderable_sidechain(e, to, module);
    Ok(json!({"module": module, "index": index, "sidechain_cleared": cleared}))
}

fn module_knobs(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.module_knobs";
    let t = first_track(e, C, p)?;
    let id = module_id(C, p)?;
    let arr = p.get("knobs").and_then(Value::as_array).ok_or_else(|| bad(C, "`knobs` must be an array of parameter ids"))?;
    if arr.len() > MAX_KNOBS {
        return Err(bad(C, format!("at most {MAX_KNOBS} knobs")));
    }
    let mut knobs: Vec<String> = Vec::with_capacity(arr.len());
    for v in arr {
        let k = v.as_str().ok_or_else(|| bad(C, "`knobs` must be an array of parameter ids"))?;
        if knobs.iter().any(|x| x == k) {
            return Err(bad(C, format!("knob `{k}` listed twice")));
        }
        knobs.push(k.to_string());
    }
    let ins = find_mut(e, C, t, id)?;
    let info = super::clap::plugin_info(&ins.plugin).ok_or_else(|| bad(C, "unknown plugin"))?;
    if let Some(k) = knobs.iter().find(|k| !info.params.iter().any(|x| x.id == k.as_str())) {
        return Err(bad(C, format!("`{}` has no parameter `{k}`", info.id)));
    }
    ins.knobs = knobs.clone();
    Ok(json!({"knobs": knobs}))
}

fn module_sidechain(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.module_sidechain";
    let t = first_track(e, C, p)?;
    let id = module_id(C, p)?;
    let source = match p.get("source") {
        None => return Err(bad(C, "`source` required (a track id or name, or null for none)")),
        Some(Value::Null) => None,
        Some(_) => track_param(e, C, p, "source")?,
    };
    let pre_fader = match p.get("pre_fader") {
        None | Some(Value::Null) => false,
        Some(v) => v.as_bool().ok_or_else(|| bad(C, "`pre_fader` must be true or false"))?,
    };
    let ins = find_mut(e, C, t, id)?;
    // A plugin this build doesn't know (a missing third-party one) keeps its setting.
    if source.is_some()
        && let Some(info) = super::clap::plugin_info(&ins.plugin)
        && !info.sidechain
    {
        return Err(bad(C, format!("`{}` takes no sidechain input", info.id)));
    }
    let want = source.map(|track| SidechainSource { track, pre_fader });
    if let Some(why) = soundcraft_mix::sidechain_refusal(e.session(), t, id, want) {
        return Err(bad(C, why));
    }
    find_mut(e, C, t, id)?.sidechain = want;
    Ok(json!({"sidechain": want}))
}

/// One line per sidechain an edit from `before` to `after` silenced, or added silent (all silent
/// ones when `before` is `None`, a session just opened), then one per linked module it made read
/// a block late (`module_link::link_report`). Cheap when no module has a source or a link group.
pub(crate) fn sidechain_report(before: Option<&soundcraft_model::Session>, after: &soundcraft_model::Session) -> Vec<String> {
    let keyed = |s: &soundcraft_model::Session| s.tracks.iter().any(|t| t.mixer.inserts.iter().any(|i| i.sidechain.is_some()));
    let mut out = Vec::new();
    if keyed(after) {
        let b = before.filter(|b| keyed(b)).map(soundcraft_mix::sidechain_routes).unwrap_or_default();
        out.extend(
            soundcraft_mix::sidechain_regressions(&b, &soundcraft_mix::sidechain_routes(after))
                .iter()
                .map(|r| soundcraft_mix::describe_route(after, r)),
        );
    }
    out.extend(super::module_link::link_report(before, after));
    out
}

/// A module copied or moved onto track `t` keeps its sidechain only when the mix engine can
/// render it there; otherwise it is cleared and the reason returned.
fn drop_unrenderable_sidechain(e: &mut Engine, t: TrackId, id: ModuleId) -> Option<String> {
    let want = e.session().track(t)?.mixer.module(id)?.sidechain?;
    let why = soundcraft_mix::sidechain_refusal(e.session(), t, id, Some(want))?;
    if let Some(ins) = e.session_mut().track_mut(t).and_then(|tr| tr.mixer.module_mut(id)) {
        ins.sidechain = None;
    }
    Some(format!("sidechain cleared: {why}"))
}

fn strip_size(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.strip_size";
    let raw = str_param(p, "size").ok_or_else(|| bad(C, "`size` required (auto, wide or slim)"))?;
    let (size, name) = match raw.trim().to_ascii_lowercase().as_str() {
        "auto" => (StripSize::Auto, "auto"),
        "wide" => (StripSize::Wide, "wide"),
        "slim" | "narrow" => (StripSize::Slim, "slim"),
        _ => return Err(bad(C, format!("unknown strip size `{raw}` (auto, wide or slim)"))),
    };
    let tracks = if bool_or(p, "all", false) { e.session().tracks.iter().map(|t| t.id).collect() } else { tracks_required(e, C, p)? };
    for t in &tracks {
        track_of(e, C, *t)?.mixer.strip_size = size;
    }
    Ok(json!({"size": name, "tracks": tracks.len()}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_time::Range;

    fn ids(e: &Engine, i: usize) -> Vec<u64> {
        e.session().tracks[i].mixer.inserts.iter().map(|x| u64::from(x.id.0)).collect()
    }

    #[test]
    fn a_failed_compound_edit_is_put_back_without_a_redo() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"format": "stereo"})).unwrap();
        let t = e.session().tracks[0].id.0;
        let a = e.execute("mix.module_add", &json!({"track": t, "plugin": "compressor"})).unwrap()["module"].as_u64().unwrap();
        let undo = e.undo_history().len();
        e.execute_merged("mix.module_remove", &json!({"track": t, "module": a}), "replace").unwrap();
        assert!(e.execute_merged("mix.module_add", &json!({"track": t, "plugin": "nope"}), "replace").is_err());
        assert!(e.undo_discard());
        assert_eq!(ids(&e, 0), [a]);
        assert_eq!(e.undo_history().len(), undo);
        assert!(!e.can_redo(), "no redo that would delete the module");
        assert!(!Engine::default().undo_discard());
    }

    #[test]
    fn module_commands_round_trip() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"format": "stereo"})).unwrap();
        let t = e.session().tracks[0].id.0;
        let a = e.execute("mix.module_add", &json!({"track": t, "plugin": "compressor"})).unwrap()["module"].as_u64().unwrap();
        let b = e.execute("mix.module_add", &json!({"track": t, "plugin": "eq_7band"})).unwrap()["module"].as_u64().unwrap();
        e.execute("mix.module_add", &json!({"track": t, "plugin": "eq_7band", "at": 0})).unwrap();
        let r = e.execute("mix.module_move", &json!({"track": t, "module": a, "to": 2})).unwrap();
        assert_eq!(r["index"], 2);
        let ids = ids(&e, 0);
        assert_eq!(&ids[1..], &[b, a]);
        let r = e.execute("mix.module_param", &json!({"track": t, "module": a, "param": "ratio", "value": 1e9})).unwrap();
        assert!(r["value"].as_f64().unwrap() < 1e9);
        assert!(e.execute("mix.module_param", &json!({"track": t, "module": 999, "param": "ratio", "value": 2})).is_err());
        assert!(e.execute("mix.module_knobs", &json!({"track": t, "module": a, "knobs": ["nope"]})).is_err());
        e.execute("mix.module_knobs", &json!({"track": t, "module": a, "knobs": ["threshold", "ratio"]})).unwrap();
        assert_eq!(e.session().tracks[0].mixer.inserts[2].knobs, ["threshold", "ratio"]);
        let five = json!(["threshold", "ratio", "attack", "release", "makeup"]);
        assert!(e.execute("mix.module_knobs", &json!({"track": t, "module": a, "knobs": five})).is_err());
        assert_eq!(e.execute("mix.module_bypass", &json!({"track": t, "module": a})).unwrap()["bypass"], true);
        assert_eq!(e.execute("mix.module_bypass", &json!({"track": t, "module": a, "value": false})).unwrap()["bypass"], false);
        let err = e.execute("mix.module_remove", &json!({"track": t, "module": 999})).unwrap_err().to_string();
        assert!(err.contains("no module 999 on"), "{err}");
        assert_eq!(e.execute("mix.module_remove", &json!({"track": t, "module": b})).unwrap()["index"], 1);
    }

    #[test]
    fn module_add_stops_at_the_cap() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let t = e.session().tracks[0].id.0;
        for _ in 0..32 {
            e.execute("mix.module_add", &json!({"track": t, "plugin": "gain"})).unwrap();
        }
        let err = e.execute("mix.module_add", &json!({"track": t, "plugin": "gain"})).unwrap_err().to_string();
        assert!(err.contains("chain is full (32 modules)"), "{err}");
        assert_eq!(ids(&e, 0).len(), 32);
    }

    #[test]
    fn module_copy_across_widths_and_undo() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"name": "M", "format": "mono"})).unwrap();
        e.execute("track.new", &json!({"name": "S", "format": "stereo"})).unwrap();
        let (m, s) = (e.session().tracks[0].id.0, e.session().tracks[1].id.0);
        let a = e.execute("mix.module_add", &json!({"track": m, "plugin": "compressor", "params": {"threshold": -30.0}})).unwrap()["module"]
            .as_u64()
            .unwrap();
        e.execute("mix.module_knobs", &json!({"track": m, "module": a, "knobs": ["ratio"]})).unwrap();
        e.execute("mix.module_bypass", &json!({"track": m, "module": a, "value": true})).unwrap();
        e.execute("mix.module_params", &json!({"track": m, "module": a, "params": {}, "preset": "Mine"})).unwrap();
        let r = e.execute("mix.module_copy", &json!({"track": m, "module": a, "to_track": s})).unwrap();
        let new = r["module"].as_u64().unwrap();
        assert_eq!(r["index"], 0);
        let chain = &e.session().tracks[1].mixer.inserts;
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].plugin, "compressor");
        assert_eq!(chain[0].params["threshold"], -30.0);
        assert!(chain[0].bypass);
        assert_eq!(chain[0].preset, "Mine");
        assert_eq!(chain[0].knobs, ["ratio"]);
        assert_eq!(u64::from(chain[0].id.0), new);
        let out = soundcraft_mix::render_range(e.session(), Range::new(0, 4096), 512);
        assert!(out.len() >= 2 && out.iter().all(|ch| ch.iter().all(|v| v.is_finite())));
        // The source keeps its module and id.
        assert_eq!(ids(&e, 0), [a]);
        assert!(e.undo());
        assert!(e.session().tracks[1].mixer.inserts.is_empty());

        // add / move / remove each undo back to the exact previous chain.
        let snap = |e: &Engine| e.session().tracks[1].mixer.inserts.clone();
        e.execute("mix.module_add", &json!({"track": s, "plugin": "gain"})).unwrap();
        let b = e.execute("mix.module_add", &json!({"track": s, "plugin": "eq_7band"})).unwrap()["module"].as_u64().unwrap();
        let before_move = snap(&e);
        e.execute("mix.module_move", &json!({"track": s, "module": b, "to": 0})).unwrap();
        assert_ne!(snap(&e), before_move);
        assert!(e.undo());
        assert_eq!(snap(&e), before_move);
        e.execute("mix.module_remove", &json!({"track": s, "module": b})).unwrap();
        assert!(e.undo());
        assert_eq!(snap(&e), before_move);
        let before_add = snap(&e);
        e.execute("mix.module_add", &json!({"track": s, "plugin": "compressor", "at": 0})).unwrap();
        assert!(e.undo());
        assert_eq!(snap(&e), before_add);
    }

    #[test]
    fn removing_a_module_drops_its_lanes_and_undo_restores_them() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let t = e.session().tracks[0].id.0;
        let a = e.execute("mix.module_add", &json!({"track": t, "plugin": "compressor"})).unwrap()["module"].as_u64().unwrap();
        let b = e.execute("mix.module_add", &json!({"track": t, "plugin": "gain"})).unwrap()["module"].as_u64().unwrap();
        for m in [a, b] {
            e.execute("automation.set_point", &json!({"track": t, "param": format!("plugin:m{m}:ratio"), "at": 0, "value": 2.0})).ok();
        }
        let lanes = |e: &Engine| e.session().tracks[0].automation.len();
        let n = lanes(&e);
        assert!(n >= 1, "set-up created no plugin lanes");
        e.execute("mix.module_remove", &json!({"track": t, "module": a})).unwrap();
        assert!(lanes(&e) < n, "the removed module's lane survived");
        assert!(e.undo());
        assert_eq!(lanes(&e), n);
    }

    #[test]
    fn removing_a_module_resets_the_view_when_any_of_its_lanes_is_shown() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let t = e.session().tracks[0].id.0;
        let a = e.execute("mix.module_add", &json!({"track": t, "plugin": "compressor"})).unwrap()["module"].as_u64().unwrap();
        for param in ["ratio", "threshold"] {
            e.execute("automation.set_point", &json!({"track": t, "param": format!("plugin:m{a}:{param}"), "at": 0, "value": 2.0})).unwrap();
        }
        assert_eq!(e.session().tracks[0].automation.len(), 2);
        let view = format!("plugin:m{a}:threshold");
        e.session_mut().tracks[0].view = view.clone();
        e.execute("mix.module_remove", &json!({"track": t, "module": a})).unwrap();
        assert_eq!(e.session().tracks[0].view, "waveform");
        assert!(e.session().tracks[0].automation.is_empty());
        assert!(e.undo());
        assert_eq!(e.session().tracks[0].view, view);
        assert_eq!(e.session().tracks[0].automation.len(), 2);
    }

    #[test]
    fn moving_a_module_to_another_track_carries_its_lanes_in_one_undo_step() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"count": 2})).unwrap();
        let (t, u) = (e.session().tracks[0].id.0, e.session().tracks[1].id.0);
        let a =
            e.execute("mix.module_add", &json!({"track": t, "plugin": "compressor", "params": {"ratio": 6.0}})).unwrap()["module"].as_u64().unwrap();
        let b = e.execute("mix.module_add", &json!({"track": t, "plugin": "gain"})).unwrap()["module"].as_u64().unwrap();
        e.execute("mix.module_add", &json!({"track": u, "plugin": "gain"})).unwrap();
        for (m, param, v) in [(a, "ratio", 2.0), (a, "threshold", -20.0), (b, "gain", 1.0)] {
            e.execute("automation.set_point", &json!({"track": t, "param": format!("plugin:m{m}:{param}"), "at": 0, "value": v})).unwrap();
        }
        e.execute("automation.set_point", &json!({"track": u, "param": "volume", "at": 0, "value": -3.0})).unwrap();
        let before = e.session().clone();
        let r = e.execute("mix.module_move", &json!({"track": t, "module": a, "to_track": u, "to": 0})).unwrap();
        let n = r["module"].as_u64().unwrap();
        assert_eq!(r["index"], 0);
        let s = e.session();
        assert_eq!(ids(&e, 0), vec![b], "gone from its track");
        assert_eq!(s.tracks[1].mixer.inserts[0].id.0, u32::try_from(n).unwrap());
        assert_eq!(s.tracks[1].mixer.inserts[0].params.get("ratio"), Some(&6.0));
        let lanes = |i: usize| -> Vec<String> { e.session().tracks[i].automation.iter().map(|l| l.param.id()).collect() };
        assert_eq!(lanes(0), vec![format!("plugin:m{b}:gain")], "the other module's lane stays");
        let mut moved = lanes(1);
        moved.sort();
        assert_eq!(moved, vec![format!("plugin:m{n}:ratio"), format!("plugin:m{n}:threshold"), "volume".to_string()]);
        let ratio = s.tracks[1].automation.iter().find(|l| l.param.id() == format!("plugin:m{n}:ratio")).unwrap();
        assert_eq!(ratio.points.len(), 1, "with its points");
        assert!(e.undo());
        assert_eq!(*e.session(), before, "one undo step");
        // A move to the same track is an ordinary move; a copy still leaves the lanes behind.
        assert_eq!(e.execute("mix.module_move", &json!({"track": t, "module": a, "to_track": t, "to": 1})).unwrap()["index"], 1);
        let c = e.execute("mix.module_copy", &json!({"track": t, "module": a, "to_track": u})).unwrap()["module"].as_u64().unwrap();
        assert!(e.session().tracks[1].automation.iter().all(|l| !l.param.id().contains(&format!("m{c}:"))));
        assert!(e.execute("mix.module_move", &json!({"track": t, "module": 99, "to_track": u, "to": 0})).is_err());
    }

    #[test]
    fn lanes_and_views_of_modules_not_in_the_chain_are_refused() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"count": 2})).unwrap();
        let (t, u) = (e.session().tracks[0].id.0, e.session().tracks[1].id.0);
        let a = e.execute("mix.module_add", &json!({"track": t, "plugin": "compressor"})).unwrap()["module"].as_u64().unwrap();
        let ghost = a + 40;
        let err = e.execute("automation.set_point", &json!({"track": t, "param": format!("plugin:m{ghost}:ratio"), "at": 0, "value": 2.0}));
        assert!(err.is_err(), "a lane for a module that isn't there");
        assert!(e.execute("track.view", &json!({"track": t, "view": format!("plugin:m{ghost}:ratio")})).is_err());
        // Another track's module is not this track's either.
        assert!(e.execute("automation.set_point", &json!({"track": u, "param": format!("plugin:m{a}:ratio"), "at": 0, "value": 2.0})).is_err());
        assert!(e.session().tracks.iter().all(|tr| tr.automation.iter().all(|l| !matches!(l.param, soundcraft_model::AutoParam::Plugin { .. }))));
        e.execute("automation.set_point", &json!({"track": t, "param": format!("plugin:m{a}:ratio"), "at": 0, "value": 2.0})).unwrap();
        e.execute("track.view", &json!({"track": t, "view": format!("plugin:m{a}:ratio")})).unwrap();
    }

    #[test]
    fn module_state_targets_modules_and_clears() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let t = e.session().tracks[0].id.0;
        let a = e.execute("mix.module_add", &json!({"track": t, "plugin": "gain"})).unwrap()["module"].as_u64().unwrap();
        let undo = e.can_undo();
        let r = e.execute("mix.module_state", &json!({"track": t, "module": a, "state": "AAAA"})).unwrap();
        // The editor's undoable write: one step, undone; over 64 KiB and junk are clean errors.
        let before = e.undo_history().len();
        e.execute("mix.module_state_edit", &json!({"track": t, "module": a, "state": "QkJC"})).unwrap();
        assert_eq!(e.undo_history().len(), before + 1);
        assert_eq!(e.undo_label(), Some("Edit Module"));
        assert!(e.execute("mix.module_state_edit", &json!({"track": t, "module": a, "state": "A".repeat(200_000)})).is_err());
        assert!(e.execute("mix.module_state_edit", &json!({"track": t, "module": a, "state": 5})).is_err());
        assert!(e.execute("mix.module_state_edit", &json!({"track": t, "module": a, "state": "!!"})).is_err());
        e.undo();
        assert_eq!(r["bytes"], 3);
        assert!(e.session().tracks[0].mixer.inserts[0].state.is_some());
        assert_eq!(e.can_undo(), undo);
        e.execute("mix.module_state", &json!({"track": t, "module": a, "state": null})).unwrap();
        assert!(e.session().tracks[0].mixer.inserts[0].state.is_none());
        assert!(e.execute("mix.module_state", &json!({"track": t, "module": 77, "state": "AAAA"})).is_err());
        assert!(e.execute("mix.module_state", &json!({"track": t, "module": "instrument", "state": "AAAA"})).is_err());
    }

    #[test]
    fn insert_aliases_use_chain_positions() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let t = e.session().tracks[0].id.0;
        for plugin in ["gain", "compressor", "eq_7band"] {
            e.execute("mix.insert", &json!({"track": t, "plugin": plugin})).unwrap();
        }
        e.execute("mix.insert_move", &json!({"track": t, "from": 0, "to": 2})).unwrap();
        let order = |e: &Engine| e.session().tracks[0].mixer.inserts.iter().map(|i| i.plugin.clone()).collect::<Vec<_>>();
        assert_eq!(order(&e), ["compressor", "eq_7band", "gain"]);
        e.execute("mix.insert_param", &json!({"track": t, "slot": "b", "param": "high_mid_freq", "value": 4321.0})).unwrap();
        assert_eq!(e.session().tracks[0].mixer.inserts[1].params["high_mid_freq"], 4321.0);
        assert!(e.execute("mix.insert", &json!({"track": t, "slot": 40, "plugin": "gain"})).is_err());
        // Every alias reaches the module at its position.
        assert_eq!(e.execute("mix.insert_bypass", &json!({"track": t, "slot": 2})).unwrap()["bypass"], true);
        e.execute("mix.insert_params", &json!({"track": t, "slot": 0, "params": {"ratio": 3.0}})).unwrap();
        e.execute("mix.insert_state", &json!({"track": t, "slot": 2, "state": "AAAA"})).unwrap();
        assert!(e.session().tracks[0].mixer.inserts[2].state.is_some());
        e.execute("mix.insert_remove", &json!({"track": t, "slot": 1})).unwrap();
        assert_eq!(order(&e), ["compressor", "gain"]);
    }

    #[test]
    fn strip_size_is_stored_and_undoable() {
        use soundcraft_model::StripSize;
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let t = e.session().tracks[0].id.0;
        assert_eq!(e.session().tracks[0].mixer.strip_size, StripSize::Auto);
        assert_eq!(e.execute("mix.strip_size", &json!({"track": t, "size": "slim"})).unwrap()["size"], "slim");
        assert_eq!(e.session().tracks[0].mixer.strip_size, StripSize::Slim);
        e.execute("mix.strip_size", &json!({"track": t, "size": "Wide"})).unwrap();
        assert_eq!(e.session().tracks[0].mixer.strip_size, StripSize::Wide);
        assert!(e.execute("mix.strip_size", &json!({"track": t, "size": "huge"})).is_err());
        assert!(e.execute("mix.strip_size", &json!({"track": t})).is_err());
    }

    #[test]
    fn strip_size_sets_many_tracks_or_all_in_one_undo_step() {
        use soundcraft_model::StripSize;
        let mut e = Engine::default();
        e.execute("track.new", &json!({"count": 3, "format": "mono"})).unwrap();
        let ids: Vec<u64> = e.session().tracks.iter().map(|t| t.id.0).collect();
        let before = e.undo_history().len();
        let r = e.execute("mix.strip_size", &json!({"tracks": [ids[0], ids[2]], "size": "slim"})).unwrap();
        assert_eq!(r["tracks"], 2);
        let sizes = |e: &Engine| e.session().tracks.iter().map(|t| t.mixer.strip_size).collect::<Vec<_>>();
        assert_eq!(sizes(&e), vec![StripSize::Slim, StripSize::Auto, StripSize::Slim]);
        assert_eq!(e.undo_history().len(), before + 1);
        e.execute("mix.strip_size", &json!({"all": true, "size": "wide"})).unwrap();
        assert!(sizes(&e).iter().all(|s| *s == StripSize::Wide));
        e.undo();
        assert_eq!(sizes(&e), vec![StripSize::Slim, StripSize::Auto, StripSize::Slim]);
        // No tracks named: the selection.
        e.execute("edit.select", &json!({"tracks": [ids[1]], "exact": true})).unwrap();
        e.execute("mix.strip_size", &json!({"size": "slim"})).unwrap();
        assert_eq!(sizes(&e), vec![StripSize::Slim, StripSize::Slim, StripSize::Slim]);
        assert!(e.undo());
        assert_eq!(e.session().tracks[0].mixer.strip_size, StripSize::Slim);
    }

    #[test]
    fn replacing_via_insert_drops_the_old_modules_lanes() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let t = e.session().tracks[0].id.0;
        let a = e.execute("mix.insert", &json!({"track": t, "plugin": "compressor"})).unwrap()["module"].as_u64().unwrap();
        e.execute("automation.set_point", &json!({"track": t, "param": format!("plugin:m{a}:ratio"), "at": 0, "value": 2.0})).ok();
        let n = e.session().tracks[0].automation.len();
        assert!(n >= 1, "set-up created no plugin lanes");
        e.execute("mix.insert", &json!({"track": t, "slot": 0, "plugin": "gain"})).unwrap();
        assert!(e.session().tracks[0].automation.len() < n);
        assert!(e.undo());
        assert_eq!(e.session().tracks[0].automation.len(), n);
    }

    // ---- derived parameters (FireStation Dist's Link and knob memory) --------------------------

    fn dist(e: &mut Engine) -> (u64, u64) {
        e.execute("track.new", &json!({"format": "stereo"})).unwrap();
        let t = e.session().tracks.last().unwrap().id.0;
        let m = e.execute("mix.module_add", &json!({"track": t, "plugin": "fs_dist"})).unwrap()["module"].as_u64().unwrap();
        (t, m)
    }

    fn val(e: &Engine, m: u64, id: &str) -> f32 {
        let ins = e.session().tracks.iter().flat_map(|t| &t.mixer.inserts).find(|i| u64::from(i.id.0) == m).unwrap();
        ins.params[id]
    }

    #[test]
    fn link_derives_level_from_drive_in_the_same_undo_step() {
        let mut e = Engine::default();
        let (t, m) = dist(&mut e);
        e.execute("mix.module_param", &json!({"track": t, "module": m, "param": "link", "value": 1})).unwrap();
        let at_noon = val(&e, m, "level");
        assert_ne!(at_noon, 0.5, "turning Link on applies it at once");
        e.execute("mix.module_param", &json!({"track": t, "module": m, "param": "drive", "value": 0.9})).unwrap();
        let w = soundcraft_dsp::plugins::fs::dist::wrapper::link_weight(0.9, 0.0, 1.0);
        assert_eq!(val(&e, m, "level"), 0.7 + (0.5 - 0.7) * w);
        assert!(e.undo());
        assert_eq!(val(&e, m, "drive"), 0.5, "one step undoes the edit...");
        assert_eq!(val(&e, m, "level"), at_noon, "...and what it derived");
        // The `mix.insert_*` aliases go through the same path.
        e.execute("mix.insert_param", &json!({"track": t, "slot": 0, "param": "drive", "value": 0.0})).unwrap();
        assert_eq!(val(&e, m, "level"), 0.7);
    }

    #[test]
    fn an_automation_lane_on_a_derived_parameter_wins() {
        let mut e = Engine::default();
        let (t, m) = dist(&mut e);
        e.execute("automation.set_point", &json!({"track": t, "param": format!("plugin:m{m}:level"), "at": 0, "value": 0.2})).unwrap();
        let level = val(&e, m, "level");
        e.execute("mix.module_params", &json!({"track": t, "module": m, "params": {"link": 1, "drive": 0.2}})).unwrap();
        assert_eq!(val(&e, m, "level"), level, "the lane owns Level");
        assert_eq!(val(&e, m, "drive"), 0.2);
    }

    #[test]
    fn mod_link_derives_level_from_depth_and_a_level_lane_wins() {
        // FireStation Mod's Link runs through the same derived-parameter layer as Dist's.
        let mut e = Engine::default();
        e.execute("track.new", &json!({"format": "stereo"})).unwrap();
        let t = e.session().tracks[0].id.0;
        let m = e.execute("mix.module_add", &json!({"track": t, "plugin": "fs_mod"})).unwrap()["module"].as_u64().unwrap();
        e.execute("mix.module_params", &json!({"track": t, "module": m, "params": {"linkLo": 0.9, "linkHi": 0.4}})).unwrap();
        let default = soundcraft_dsp::plugin_info("fs_mod").unwrap().param("level").unwrap().default;
        assert_eq!(val(&e, m, "level"), default, "Link off: Level untouched");
        e.execute("mix.module_param", &json!({"track": t, "module": m, "param": "link", "value": 1})).unwrap();
        let w = |d: f32| soundcraft_dsp::plugins::fs::mod_engine::link_weight(d);
        assert_eq!(val(&e, m, "level"), 0.9 + (0.4 - 0.9) * w(0.5), "turning Link on applies it");
        e.execute("mix.module_param", &json!({"track": t, "module": m, "param": "depth", "value": 0.9})).unwrap();
        assert_eq!(val(&e, m, "level"), 0.9 + (0.4 - 0.9) * w(0.9));
        assert!(e.undo());
        assert_eq!(val(&e, m, "depth"), 0.5, "one step undoes the edit...");
        assert_eq!(val(&e, m, "level"), 0.9 + (0.4 - 0.9) * w(0.5), "...and what it derived");
        e.execute("automation.set_point", &json!({"track": t, "param": format!("plugin:m{m}:level"), "at": 0, "value": 0.2})).unwrap();
        let level = val(&e, m, "level");
        e.execute("mix.module_param", &json!({"track": t, "module": m, "param": "depth", "value": 0.1})).unwrap();
        assert_eq!(val(&e, m, "level"), level, "the lane owns Level");
    }

    #[test]
    fn circuit_changes_swap_knob_memory_kept_with_the_module() {
        let mut e = Engine::default();
        let (t, m) = dist(&mut e);
        e.execute("mix.module_params", &json!({"track": t, "module": m, "params": {"drive": 0.66, "tone": 0.11}})).unwrap();
        e.execute("mix.module_param", &json!({"track": t, "module": m, "param": "circuit", "value": 3})).unwrap();
        assert_eq!((val(&e, m, "drive"), val(&e, m, "tone")), (0.85, 0.6), "Fuzz Face's start point");
        e.execute("mix.module_param", &json!({"track": t, "module": m, "param": "circuit", "value": 0})).unwrap();
        assert_eq!((val(&e, m, "drive"), val(&e, m, "tone")), (0.66, 0.11), "the Tube Screamer's own knobs");
        // The memory travels with the module (session file, copy).
        let ins = e.session().tracks[0].mixer.inserts[0].clone();
        assert_eq!(ins.memory.get("0.drive"), Some(&0.66));
        let text = serde_json::to_string(&ins).unwrap();
        let back: soundcraft_model::Insert = serde_json::from_str(&text).unwrap();
        assert_eq!(back.memory, ins.memory);
        // Undo puts the memory back too.
        assert!(e.undo());
        assert!(e.undo());
        assert!(e.session().tracks[0].mixer.inserts[0].memory.is_empty());
    }

    #[test]
    fn module_params_memory_restores_the_bank_without_a_knob_swap() {
        let mut e = Engine::default();
        let (t, m) = dist(&mut e);
        e.execute("mix.module_params", &json!({"track": t, "module": m, "params": {"drive": 0.66, "tone": 0.11}})).unwrap();
        // A snapshot: circuit 3 with its own knobs, and a bank where the Tube Screamer holds 0.2 / 0.3.
        let bank = json!({"0.drive": 0.2, "0.tone": 0.3, "0.level": 0.5, "3.drive": 0.4, "3.tone": 0.45, "3.level": 0.5});
        let snap = json!({"track": t, "module": m, "params": {"circuit": 3, "drive": 0.4, "tone": 0.45}, "memory": bank});
        e.execute("mix.module_params", &snap).unwrap();
        assert_eq!((val(&e, m, "drive"), val(&e, m, "tone")), (0.4, 0.45), "the snapshot's knobs, no recall from the bank");
        assert_eq!(e.session().tracks[0].mixer.inserts[0].memory.get("0.drive"), Some(&0.2), "the bank is the snapshot's");
        e.execute("mix.module_param", &json!({"track": t, "module": m, "param": "circuit", "value": 0})).unwrap();
        assert_eq!((val(&e, m, "drive"), val(&e, m, "tone")), (0.2, 0.3), "flipping back recalls the restored bank");
        assert!(e.undo());
        assert!(e.undo());
        assert_eq!(e.session().tracks[0].mixer.inserts[0].memory.get("0.drive"), None, "undo puts the old bank back");
        assert_eq!((val(&e, m, "drive"), val(&e, m, "tone")), (0.66, 0.11), "and the old knobs");
        for bad in [json!([1]), json!({"k": "x"}), json!({"k": null})] {
            let r = e.execute("mix.module_params", &json!({"track": t, "module": m, "params": {}, "memory": bad}));
            assert!(r.is_err(), "hostile memory is refused: {bad}");
        }
    }

    #[test]
    fn module_add_params_run_the_derived_parameters() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"format": "stereo"})).unwrap();
        let t = e.session().tracks[0].id.0;
        let m = e.execute("mix.module_add", &json!({"track": t, "plugin": "fs_dist", "params": {"link": 1, "drive": 0.9}})).unwrap()["module"]
            .as_u64()
            .unwrap();
        let w = soundcraft_dsp::plugins::fs::dist::wrapper::link_weight(0.9, 0.0, 1.0);
        assert_eq!(val(&e, m, "level"), 0.7 + (0.5 - 0.7) * w, "Link applied at creation");
    }

    #[test]
    fn a_lane_only_locks_while_the_track_reads_automation() {
        let mut e = Engine::default();
        let (t, m) = dist(&mut e);
        e.execute("automation.set_point", &json!({"track": t, "param": format!("plugin:m{m}:level"), "at": 0, "value": 0.2})).unwrap();
        e.session_mut().tracks[0].mixer.automation_mode = soundcraft_model::AutomationMode::Off;
        let level = val(&e, m, "level");
        e.execute("mix.module_params", &json!({"track": t, "module": m, "params": {"link": 1, "drive": 0.2}})).unwrap();
        assert_ne!(val(&e, m, "level"), level, "automation off: the lane drives nothing, Link sets Level");
    }

    #[test]
    fn a_preset_recall_gets_the_matched_level_and_no_swap() {
        let mut e = Engine::default();
        let (t, m) = dist(&mut e);
        e.execute("mix.module_params", &json!({"track": t, "module": m, "params": {"circuit": 2, "drive": 0.9, "tone": 0.2}, "preset": "Wall"}))
            .unwrap();
        assert_eq!((val(&e, m, "drive"), val(&e, m, "tone")), (0.9, 0.2), "no recall on a preset");
        let w = soundcraft_dsp::plugins::fs::dist::wrapper::link_weight(0.9, 0.15, 1.0);
        assert_eq!(val(&e, m, "level"), 0.6 + (0.4 - 0.6) * w);
        // Plugins without derived parameters are untouched.
        let c = e.execute("mix.module_add", &json!({"track": t, "plugin": "compressor"})).unwrap()["module"].as_u64().unwrap();
        e.execute("mix.module_param", &json!({"track": t, "module": c, "param": "ratio", "value": 4})).unwrap();
        assert!(e.session().tracks[0].mixer.inserts[1].memory.is_empty());
    }

    /// Tracks A (mono), B (stereo) and an aux X; a sidechain-taking module (a test id this
    /// build doesn't know, so its setting is kept) on A, B and X.
    fn sidechain_session() -> (Engine, [u64; 3], [u64; 3]) {
        let mut e = Engine::default();
        for (name, kind) in [("A", "audio"), ("B", "audio"), ("X", "aux")] {
            e.execute("track.new", &json!({"name": name, "kind": kind})).unwrap();
        }
        let t: Vec<u64> = e.session().tracks.iter().map(|t| t.id.0).collect();
        let mut m = [0u64; 3];
        for (i, tr) in e.session_mut().tracks.iter_mut().enumerate() {
            m[i] = u64::from(tr.mixer.add_module(soundcraft_model::Insert::new("clap:test.key"), None).unwrap().0);
        }
        (e, [t[0], t[1], t[2]], m)
    }

    fn sidechain_of(e: &Engine, i: usize) -> Option<soundcraft_model::SidechainSource> {
        e.session().tracks[i].mixer.inserts[0].sidechain
    }

    #[test]
    fn module_sidechain_sets_clears_undoes_and_saves() {
        let (mut e, [a, b, _], [ma, ..]) = sidechain_session();
        let r = e.execute("mix.module_sidechain", &json!({"track": a, "module": ma, "source": "B", "pre_fader": true})).unwrap();
        assert_eq!(r["sidechain"], json!({"track": b, "pre_fader": true}));
        let want = Some(soundcraft_model::SidechainSource { track: soundcraft_model::TrackId(b), pre_fader: true });
        assert_eq!(sidechain_of(&e, 0), want);
        // Saved and loaded with the session.
        let saved = serde_json::to_string(e.session()).unwrap();
        let loaded: soundcraft_model::Session = serde_json::from_str(&saved).unwrap();
        assert_eq!(loaded.tracks[0].mixer.inserts[0].sidechain, want);
        // One undo step each way.
        assert!(e.undo());
        assert_eq!(sidechain_of(&e, 0), None);
        assert!(e.redo());
        assert_eq!(sidechain_of(&e, 0), want);
        assert_eq!(e.execute("mix.module_sidechain", &json!({"track": a, "module": ma, "source": null})).unwrap()["sidechain"], json!(null));
        assert_eq!(sidechain_of(&e, 0), None);
        assert!(e.undo());
        assert_eq!(sidechain_of(&e, 0), want);
    }

    #[test]
    fn module_sidechain_refuses_cycles_and_what_cannot_render() {
        let (mut e, [a, b, x], [ma, mb, mx]) = sidechain_session();
        e.execute("mix.module_sidechain", &json!({"track": a, "module": ma, "source": b})).unwrap();
        let undo = e.undo_history().len();
        let err = |e: &mut Engine, p: serde_json::Value| e.execute("mix.module_sidechain", &p).unwrap_err().to_string();
        // B keyed from A closes the loop A ← B ← A; a track can't key itself.
        assert!(err(&mut e, json!({"track": b, "module": mb, "source": a})).contains("cycle"));
        assert!(err(&mut e, json!({"track": b, "module": mb, "source": b})).contains("cycle"));
        assert_eq!(sidechain_of(&e, 1), None, "a refused sidechain is not stored");
        assert_eq!(e.undo_history().len(), undo, "and leaves no undo step");
        // A track can't wait for an aux; an aux can take a key from a track.
        assert!(err(&mut e, json!({"track": b, "module": mb, "source": x})).contains("aux"));
        e.execute("mix.module_sidechain", &json!({"track": x, "module": mx, "source": a})).unwrap();
        // Bad parameters.
        assert!(err(&mut e, json!({"track": a, "module": ma})).contains("`source` required"));
        assert!(err(&mut e, json!({"track": a, "module": ma, "source": 999_999})).contains("no track"));
        assert!(err(&mut e, json!({"track": a, "module": ma, "source": b, "pre_fader": "yes"})).contains("pre_fader"));
        assert!(err(&mut e, json!({"track": a, "module": 999, "source": b})).contains("no module"));
        // A plugin that takes no sidechain.
        let c = e.execute("mix.module_add", &json!({"track": a, "plugin": "compressor"})).unwrap()["module"].as_u64().unwrap();
        assert!(err(&mut e, json!({"track": a, "module": c, "source": b})).contains("takes no sidechain"));
        // Rendering what the commands built (and a deleted source) stays finite.
        e.execute("track.delete", &json!({"tracks": [b]})).unwrap();
        let out = soundcraft_mix::render_range(e.session(), Range::new(0, 4096), 512);
        assert!(out.iter().flatten().all(|v| v.is_finite()));
    }

    #[test]
    fn module_sidechain_refuses_what_would_silence_an_older_key() {
        // B keyed from A first; then A keyed from B passes its own check (A resolves first) but
        // would turn B's working key into a cycle: refused.
        let (mut e, [a, b, _], [ma, mb, _]) = sidechain_session();
        e.execute("mix.module_sidechain", &json!({"track": b, "module": mb, "source": a})).unwrap();
        let err = e.execute("mix.module_sidechain", &json!({"track": a, "module": ma, "source": b})).unwrap_err().to_string();
        assert!(err.contains("cycle") && err.contains("silence another sidechain"), "{err}");
        assert_eq!(sidechain_of(&e, 0), None);
        assert!(soundcraft_mix::sidechain_routes(e.session()).iter().all(|r| r.result.is_ok()));
    }

    #[test]
    fn copies_and_moves_drop_a_sidechain_they_cannot_render() {
        let (mut e, [a, b, _], [ma, mb, _]) = sidechain_session();
        e.execute("mix.module_sidechain", &json!({"track": a, "module": ma, "source": b})).unwrap();
        // A copy of A's module onto B would key B from itself.
        let r = e.execute("mix.module_copy", &json!({"track": a, "module": ma, "to_track": b})).unwrap();
        assert!(r["sidechain_cleared"].as_str().unwrap().contains("cycle"), "{r}");
        let copy = soundcraft_model::ModuleId(u32::try_from(r["module"].as_u64().unwrap()).unwrap());
        assert_eq!(e.session().tracks[1].mixer.module(copy).unwrap().sidechain, None);
        // A copy that can render keeps it (onto the aux).
        let x = e.session().tracks[2].id.0;
        let r = e.execute("mix.module_copy", &json!({"track": a, "module": ma, "to_track": x})).unwrap();
        assert_eq!(r["sidechain_cleared"], json!(null));
        // Moving B's own module onto A is fine; moving A's keyed module onto B is not.
        let r = e.execute("mix.module_move", &json!({"track": a, "module": ma, "to": 0, "to_track": b})).unwrap();
        assert!(r["sidechain_cleared"].is_string(), "{r}");
        let _ = mb;
        assert!(soundcraft_mix::sidechain_routes(e.session()).iter().all(|r| r.result.is_ok()));
    }

    #[test]
    fn edits_that_silence_a_key_report_it() {
        let (mut e, [a, b, x], [_, mb, mx]) = sidechain_session();
        e.execute("mix.module_sidechain", &json!({"track": b, "module": mb, "source": a})).unwrap();
        // Deactivating the source: reported in the result and the messages.
        let r = e.execute("track.make_inactive", &json!({"tracks": [a], "inactive": true})).unwrap();
        let w = r["sidechain_warnings"].as_array().unwrap();
        assert!(w[0].as_str().unwrap().contains("inactive"), "{r}");
        assert!(e.messages.last().unwrap().contains("silent"));
        let r = e.execute("track.make_inactive", &json!({"tracks": [a], "inactive": false})).unwrap();
        assert!(r.get("sidechain_warnings").is_none(), "nothing newly silent: {r}");
        // A routing change that closes a loop: aux Y reads bus B1, X is keyed from Y, then X
        // sends to B1 (X → B1 → Y → X).
        e.execute("track.new", &json!({"name": "Y", "kind": "aux"})).unwrap();
        let s = e.session_mut();
        let b1 = s.add_bus("B1", soundcraft_model::ChannelFormat::Stereo);
        s.tracks[3].mixer.input = soundcraft_model::Route::Bus(b1);
        let y = s.tracks[3].id.0;
        e.execute("mix.module_sidechain", &json!({"track": x, "module": mx, "source": y})).unwrap();
        let r = e.execute("mix.send", &json!({"track": x, "slot": 0, "bus": "B1"})).unwrap();
        assert!(r["sidechain_warnings"][0].as_str().unwrap().contains("cycle"), "{r}");
        // Deleting a source.
        let r = e.execute("track.delete", &json!({"tracks": [a]})).unwrap();
        assert!(r["sidechain_warnings"][0].as_str().unwrap().contains("does not exist"), "{r}");
    }

    #[test]
    fn a_session_that_loads_with_silent_keys_says_so() {
        let (mut e, [a, b, _], [ma, mb, _]) = sidechain_session();
        let mut s = e.session().clone();
        // A hand-edited file with a loop: A ← B ← A.
        s.tracks[0].mixer.module_mut(soundcraft_model::ModuleId(u32::try_from(ma).unwrap())).unwrap().sidechain =
            Some(soundcraft_model::SidechainSource { track: soundcraft_model::TrackId(b), pre_fader: false });
        s.tracks[1].mixer.module_mut(soundcraft_model::ModuleId(u32::try_from(mb).unwrap())).unwrap().sidechain =
            Some(soundcraft_model::SidechainSource { track: soundcraft_model::TrackId(a), pre_fader: false });
        e.replace_session(s);
        assert!(e.messages.last().unwrap().contains("cycle"), "{:?}", e.messages);
        // The next command carries the note once (the UI shows it in the status line).
        let r = e.execute("track.new", &json!({})).unwrap();
        assert!(r["sidechain_warnings"][0].as_str().unwrap().contains("cycle"), "{r}");
        let r = e.execute("track.new", &json!({})).unwrap();
        assert!(r.get("sidechain_warnings").is_none());
    }
}
