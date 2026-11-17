//! Modulators on a track (`soundcraft_model::Modulator`): an LFO or an envelope follower that
//! moves any of the track's chain modules' parameters, run by the mix every block. At most
//! [`MAX_MODULATORS`] per track, each with at most [`MAX_MOD_TARGETS`] targets. Each command is
//! one undo step (a drag in the window's controls is merged into one by the UI).

use super::module::first_track;
use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{LFO_SHAPES, LFO_SYNC, MAX_MOD_TARGETS, MAX_MODULATORS, ModTarget, Modulator, ModulatorKind, ModuleId};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.modulator_add",
            "Add Modulator",
            [],
            None,
            "{track?, kind: lfo | envelope} — a modulator on the track (at most 8) → {modulator: id}",
            has_selection,
            add
        ),
        cmd!(
            "mix.modulator_set",
            "Set Modulator",
            [],
            None,
            "{track?, modulator: id, bypass?, shape?: 0..4 (sine triangle saw square random), rate?: 0.01..40 Hz, sync?: 0 (free) | 1..9 (4 bars 2 bars 1 bar 1/2 1/4 1/8 1/8T 1/16 1/32), attack?: 0.1..2000 ms, release?: 0.1..2000 ms} → {modulator}",
            has_selection,
            set
        ),
        cmd!(
            "mix.modulator_target",
            "Modulate Parameter",
            [],
            None,
            "{track?, modulator: id, module: id, param, depth: -1..1} — the modulator moves the module's parameter by `depth` of its range (0 stops it; at most 8 per modulator) → {modulator}",
            has_selection,
            target
        ),
        cmd!(
            "mix.modulator_remove",
            "Remove Modulator",
            [],
            None,
            "{track?, modulator: id} — takes the modulator off the track → {removed}",
            has_selection,
            remove
        ),
    ]
}

fn modulator_id(c: &str, p: &Value) -> Result<u32> {
    p.get("modulator").and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok()).ok_or_else(|| bad(c, "`modulator`: which modulator (its id)"))
}

fn find<'a>(e: &'a mut Engine, c: &str, t: TrackId, id: u32) -> Result<&'a mut Modulator> {
    e.session_mut()
        .track_mut(t)
        .and_then(|tr| tr.modulators.iter_mut().find(|m| m.id == id))
        .ok_or_else(|| bad(c, format!("no modulator {id} on that track")))
}

fn add(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.modulator_add";
    let t = first_track(e, C, p)?;
    let kind = str_param(p, "kind").and_then(ModulatorKind::from_key).ok_or_else(|| bad(C, "`kind` is lfo or envelope"))?;
    let tr = e.session_mut().track_mut(t).ok_or_else(|| bad(C, "no such track"))?;
    if tr.modulators.len() >= MAX_MODULATORS {
        return Err(bad(C, format!("`{}` already has {MAX_MODULATORS} modulators", tr.name)));
    }
    let id = tr.modulators.iter().map(|m| m.id).max().map_or(1, |m| m.saturating_add(1));
    tr.modulators.push(Modulator::new(id, kind));
    Ok(json!({"modulator": id}))
}

fn set(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.modulator_set";
    let t = first_track(e, C, p)?;
    let id = modulator_id(C, p)?;
    let num = |key: &str, lo: f64, hi: f64| -> Result<Option<f64>> {
        match p.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => {
                v.as_f64().filter(|x| x.is_finite() && (lo..=hi).contains(x)).map(Some).ok_or_else(|| bad(C, format!("`{key}` is {lo} to {hi}")))
            }
        }
    };
    let shape = num("shape", 0.0, (LFO_SHAPES.len() - 1) as f64)?;
    let rate = num("rate", 0.01, 40.0)?;
    let sync = num("sync", 0.0, LFO_SYNC.len() as f64)?;
    let attack = num("attack", 0.1, 2000.0)?;
    let release = num("release", 0.1, 2000.0)?;
    let bypass = match p.get("bypass") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_bool().ok_or_else(|| bad(C, "`bypass` is true or false"))?),
    };
    let m = find(e, C, t, id)?;
    if let Some(v) = shape {
        m.shape = v.round() as u8;
    }
    if let Some(v) = rate {
        m.rate_hz = v as f32;
    }
    if let Some(v) = sync {
        m.sync = v.round() as u8;
    }
    if let Some(v) = attack {
        m.attack_ms = v as f32;
    }
    if let Some(v) = release {
        m.release_ms = v as f32;
    }
    m.bypass = bypass.unwrap_or(m.bypass);
    m.sane();
    Ok(json!({"modulator": m.clone()}))
}

fn target(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.modulator_target";
    let t = first_track(e, C, p)?;
    let id = modulator_id(C, p)?;
    let module = p
        .get("module")
        .and_then(Value::as_u64)
        .and_then(|v| u32::try_from(v).ok())
        .map(ModuleId)
        .ok_or_else(|| bad(C, "`module`: a chain module's id"))?;
    let param = str_param(p, "param").ok_or_else(|| bad(C, "`param`: the parameter's id"))?.to_string();
    let depth = p.get("depth").and_then(Value::as_f64).filter(|d| d.is_finite()).ok_or_else(|| bad(C, "`depth` is -1 to 1"))?.clamp(-1.0, 1.0) as f32;
    let ins = e.session().track(t).and_then(|tr| tr.module(module)).ok_or_else(|| bad(C, "no such module on that track"))?;
    let known = super::clap::plugin_info(&ins.plugin).is_some_and(|i| i.param(&param).is_some());
    if !known {
        return Err(bad(C, format!("`{}` has no parameter `{param}`", ins.plugin)));
    }
    let m = find(e, C, t, id)?;
    match m.targets.iter().position(|x| x.module == module && x.param == param) {
        Some(i) if depth == 0.0 => {
            m.targets.remove(i);
        }
        Some(i) => {
            if let Some(x) = m.targets.get_mut(i) {
                x.depth = depth;
            }
        }
        None if depth == 0.0 => {}
        None => {
            if m.targets.len() >= MAX_MOD_TARGETS {
                return Err(bad(C, format!("a modulator moves at most {MAX_MOD_TARGETS} parameters")));
            }
            m.targets.push(ModTarget { module, param, depth });
        }
    }
    Ok(json!({"modulator": m.clone()}))
}

fn remove(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.modulator_remove";
    let t = first_track(e, C, p)?;
    let id = modulator_id(C, p)?;
    let tr = e.session_mut().track_mut(t).ok_or_else(|| bad(C, "no such track"))?;
    let n = tr.modulators.len();
    tr.modulators.retain(|m| m.id != id);
    if tr.modulators.len() == n {
        return Err(bad(C, format!("no modulator {id} on that track")));
    }
    Ok(json!({"removed": id}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;

    #[test]
    fn modulators_are_added_set_aimed_and_removed() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"format": "stereo"})).unwrap();
        let t = e.session().tracks[0].id.0;
        let f = e.execute("mix.module_add", &json!({"track": t, "plugin": "fs_filter"})).unwrap()["module"].as_u64().unwrap();
        let m = e.execute("mix.modulator_add", &json!({"track": t, "kind": "lfo"})).unwrap()["modulator"].as_u64().unwrap();
        e.execute("mix.modulator_set", &json!({"track": t, "modulator": m, "rate": 4.0, "shape": 1, "sync": 5})).unwrap();
        e.execute("mix.modulator_target", &json!({"track": t, "modulator": m, "module": f, "param": "cutoff", "depth": 0.4})).unwrap();
        let md = &e.session().tracks[0].modulators[0];
        assert_eq!((md.rate_hz, md.shape, md.sync, md.targets.len()), (4.0, 1, 5, 1));
        assert_eq!(md.targets[0].depth, 0.4);
        // a parameter the plug-in lacks, a bad rate, a missing modulator: refused
        assert!(e.execute("mix.modulator_target", &json!({"track": t, "modulator": m, "module": f, "param": "nope", "depth": 0.4})).is_err());
        assert!(e.execute("mix.modulator_set", &json!({"track": t, "modulator": m, "rate": 400.0})).is_err());
        assert!(e.execute("mix.modulator_set", &json!({"track": t, "modulator": 99, "rate": 1.0})).is_err());
        // depth 0 stops it; undo brings it back
        e.execute("mix.modulator_target", &json!({"track": t, "modulator": m, "module": f, "param": "cutoff", "depth": 0.0})).unwrap();
        assert!(e.session().tracks[0].modulators[0].targets.is_empty());
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().tracks[0].modulators[0].targets.len(), 1);
        let n = e.execute("mix.modulator_add", &json!({"track": t, "kind": "envelope"})).unwrap()["modulator"].as_u64().unwrap();
        assert_ne!(n, m);
        e.execute("mix.modulator_remove", &json!({"track": t, "modulator": m})).unwrap();
        assert_eq!(e.session().tracks[0].modulators.len(), 1);
        for _ in 0..7 {
            e.execute("mix.modulator_add", &json!({"track": t, "kind": "lfo"})).unwrap();
        }
        assert!(e.execute("mix.modulator_add", &json!({"track": t, "kind": "lfo"})).is_err(), "at most 8");
    }
}
