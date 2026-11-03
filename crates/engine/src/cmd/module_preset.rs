//! Loading a preset into a module as one undo step (`mix.module_preset`): a hosted plugin's
//! preset as its state (a VST3 `.vstpreset`, a CLAP discovered preset, saved by the UI), or an
//! instrument's preset as its parameters (a built-in's preset, a VST3 program). Chain modules' built-in presets go through
//! `mix.module_params {params, preset}` (which also recalls derived parameters).

use super::*;
use serde_json::json;
use soundcraft_model::{AutoParam, Insert, b64};
use std::sync::atomic::{AtomicU64, Ordering};

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "mix.module_preset",
        label: "Load Preset",
        menu: &[],
        shortcut: None,
        params: "{track, module: id | \"instrument\", preset: name, state?: base64 (a hosted plugin's state, as `mix.module_state` stores it), params?: {id: value} (an instrument's or a hosted plugin's parameters, clamped; a hosted plugin's program)} → {bytes, set}. A hosted preset clears the module's stored parameter values except those an automation lane drives. Undoable; a hosted plugin's running instance is made again from the new state (and from the old one on undo). Not journaled (states can be large).",
        enabled: has_tracks,
        run: module_preset,
        journal: false,
        undoable: true,
        view: false,
    }]
}

const C: &str = "mix.module_preset";

/// Largest state taken (as the hosts read them).
const MAX_STATE_BYTES: usize = 64 << 20;

/// A fresh `Insert::state_load` value (never 0, never one handed out before in this process).
fn next_load() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed).max(1)
}

fn module_preset(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = track_param(e, C, p, "track")?.ok_or_else(|| bad(C, "`track` required"))?;
    let name = str_param(p, "preset").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(C, "`preset` (its name) required"))?.to_string();
    let state = match p.get("state") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.len() > MAX_STATE_BYTES.div_ceil(3) * 4 + 8 => return Err(bad(C, "`state` is too large")),
        Some(Value::String(s)) => Some(b64::decode(s).filter(|b| !b.is_empty()).ok_or_else(|| bad(C, "`state` is not valid base64"))?),
        Some(_) => return Err(bad(C, "`state` must be a base64 string")),
    };
    let params = match p.get("params") {
        None | Some(Value::Null) => None,
        Some(Value::Object(m)) => Some(m.clone()),
        Some(_) => return Err(bad(C, "`params` must be an object")),
    };
    let instrument = p.get("module").and_then(Value::as_str).is_some_and(|s| s.trim().eq_ignore_ascii_case("instrument"));
    let id = if instrument { None } else { Some(super::module::module_id(C, p)?) };
    let tr = e.session_mut().track_mut(t).ok_or_else(|| bad(C, "no track"))?;
    // The module's parameters an automation lane drives (the instrument has no lanes).
    let laned: Vec<String> = match id {
        Some(m) => tr
            .automation
            .iter()
            .filter_map(|l| match &l.param {
                AutoParam::Plugin { module, param } if *module == m => Some(param.clone()),
                _ => None,
            })
            .collect(),
        None => Vec::new(),
    };
    let ins: &mut Insert = match id {
        None => tr.instrument.as_mut().ok_or_else(|| bad(C, "the track has no instrument"))?,
        Some(m) => tr.mixer.module_mut(m).ok_or_else(|| bad(C, "no such module"))?,
    };
    let hosted = soundcraft_mix::is_third_party(&ins.plugin);
    let mut set = 0;
    match (state, params) {
        (Some(bytes), None) if hosted => {
            ins.set_state_bytes(&bytes);
            ins.state_load = next_load();
            // The preset is the plugin's whole sound: stored values from before it (a program
            // picked earlier included) would be applied over it on the next resend or rebuild.
            // Values an automation lane drives stay.
            ins.params.retain(|k, _| laned.contains(k));
        }
        (Some(_), _) => return Err(bad(C, "only a hosted plugin's preset is a `state`")),
        (None, Some(values)) if id.is_none() || hosted => {
            // A hosted plugin's values cannot all be checked without its description (it may not
            // be created yet); finite numbers are taken, clamped when the description is known.
            let info = super::clap::plugin_info(&ins.plugin);
            if info.is_none() && !hosted {
                return Err(bad(C, format!("unknown plugin `{}`", ins.plugin)));
            }
            let mut checked = Vec::with_capacity(values.len());
            for (k, v) in &values {
                let x = v.as_f64().map(|x| x as f32).filter(|x| x.is_finite()).ok_or_else(|| bad(C, format!("`{k}` must be a number")))?;
                let x = match info {
                    Some(info) => info.param(k).ok_or_else(|| bad(C, format!("`{}` has no parameter `{k}`", info.id)))?.clamp(x),
                    None => x,
                };
                checked.push((k.clone(), x));
            }
            set = checked.len();
            ins.params.extend(checked);
            if hosted {
                // A program change lives in the plugin's state: undo loads the state back.
                ins.state_load = next_load();
            }
        }
        (None, Some(_)) => return Err(bad(C, "a built-in chain module's preset parameters go through mix.module_params")),
        (None, None) => return Err(bad(C, "a preset needs a `state` (hosted plugins) or `params` (instruments)")),
    }
    ins.preset = name;
    let bytes = ins.state_bytes().map_or(0, |b| b.len());
    Ok(json!({"bytes": bytes, "set": set}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::{AutoParam, Insert, TrackId, b64};

    /// An audio track with a hosted module (not installed: nothing is created) and an instrument
    /// track playing the built-in synth.
    fn engine() -> (Engine, u64, u64, u64) {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"name": "A"})).unwrap();
        e.execute("track.new", &json!({"name": "K", "kind": "instrument"})).unwrap();
        let (a, k) = (e.session().tracks[0].id, e.session().tracks[1].id);
        let m = e.session_mut().track_mut(a).unwrap().mixer.add_module(Insert::new("vst3:0123456789ABCDEF0123456789ABCDEF"), None).unwrap();
        (e, a.0, k.0, u64::from(m.0))
    }

    fn module(e: &Engine, t: u64) -> &Insert {
        &e.session().track(TrackId(t)).unwrap().mixer.inserts[0]
    }

    #[test]
    fn a_hosted_preset_is_the_state_in_one_undo_step() {
        let (mut e, a, _, m) = engine();
        e.execute("mix.module_state", &json!({"track": a, "module": m, "state": b64::encode(b"before")})).unwrap();
        let undo = e.undo_history().len();
        let r = e.execute("mix.module_preset", &json!({"track": a, "module": m, "preset": "Warm", "state": b64::encode(b"after!")})).unwrap();
        assert_eq!(r["bytes"], 6);
        let ins = module(&e, a);
        assert_eq!((ins.state_bytes().unwrap(), ins.preset.as_str()), (b"after!".to_vec(), "Warm"));
        let load = ins.state_load;
        assert_ne!(load, 0, "the running instance is made again");
        assert_eq!(e.undo_history().len(), undo + 1);
        assert_eq!(e.undo_label(), Some("Load Preset"));
        e.undo();
        let ins = module(&e, a);
        assert_eq!((ins.state_bytes().unwrap(), ins.state_load), (b"before".to_vec(), 0));
        e.redo();
        assert_eq!(module(&e, a).state_load, load);
        // A second load is a new loading, even of the same preset.
        e.execute("mix.module_preset", &json!({"track": a, "module": m, "preset": "Warm", "state": b64::encode(b"after!")})).unwrap();
        assert_ne!(module(&e, a).state_load, load);
        assert!(!e.journal.iter().any(|(id, _)| id == "mix.module_preset"), "not journaled");
    }

    /// A built-in compressor behind a hosted id, logging the parameters it is sent.
    struct Logged {
        inner: Box<dyn soundcraft_dsp::Plugin>,
        sent: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl soundcraft_dsp::Plugin for Logged {
        fn info(&self) -> &'static soundcraft_dsp::PluginInfo {
            self.inner.info()
        }
        fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
            self.inner.prepare(sr, mb, ch);
        }
        fn reset(&mut self) {
            self.inner.reset();
        }
        fn set_param(&mut self, id: &str, v: f32) -> bool {
            self.sent.lock().unwrap().push(id.to_string());
            self.inner.set_param(id, v)
        }
        fn param(&self, id: &str) -> Option<f32> {
            self.inner.param(id)
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
            self.inner.process(io, frames);
        }
    }

    /// A mix engine for `e`'s session with a logging instance in module `m` of track `t`.
    fn live(e: &Engine, t: u64, m: u64) -> (soundcraft_mix::MixEngine, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut eng = soundcraft_mix::MixEngine::new(48_000.0, 512);
        eng.set_external_instances(true);
        eng.adopt(vec![soundcraft_mix::PreparedInstance {
            track: TrackId(t),
            slot: soundcraft_mix::SlotKey::Module(soundcraft_model::ModuleId(m as u32)),
            id: module(e, t).plugin.clone(),
            ch: soundcraft_mix::instance_specs(e.session()).first().map_or(1, |s| s.ch),
            plugin: Box::new(Logged { inner: soundcraft_dsp::create("compressor").unwrap(), sent: std::sync::Arc::clone(&sent) }),
        }]);
        (eng, sent)
    }

    fn render(eng: &mut soundcraft_mix::MixEngine, e: &Engine) {
        let mut out = vec![vec![0.0f32; 512]; 2];
        eng.render(e.session(), 0, 512, &mut out);
    }

    /// Stored values from before a hosted preset are not sent over it: not when another value is
    /// stored afterwards (which resends the set), nor to an instance made anew (a rebuild, an
    /// offline render, a reopened session).
    #[test]
    fn a_hosted_preset_drops_the_stored_values_from_before_it() {
        let (mut e, a, _, m) = engine();
        let id = soundcraft_model::ModuleId(m as u32);
        let ins = e.session_mut().track_mut(TrackId(a)).unwrap().mixer.module_mut(id).unwrap();
        ins.params.insert("threshold".into(), -30.0);
        ins.params.insert("ratio".into(), 4.0);
        let (mut eng, sent) = live(&e, a, m);
        render(&mut eng, &e);
        assert!(sent.lock().unwrap().contains(&"threshold".to_string()));
        sent.lock().unwrap().clear();
        e.execute("mix.module_preset", &json!({"track": a, "module": m, "preset": "P", "state": "AAAA"})).unwrap();
        assert!(module(&e, a).params.is_empty());
        render(&mut eng, &e);
        e.session_mut().track_mut(TrackId(a)).unwrap().mixer.module_mut(id).unwrap().params.insert("attack".into(), 5.0);
        render(&mut eng, &e);
        assert_eq!(*sent.lock().unwrap(), ["attack"], "only the value stored after the preset");
        // Made anew (the offline renderer and a rebuilt engine start from the stored values).
        let (mut fresh, sent) = live(&e, a, m);
        render(&mut fresh, &e);
        assert_eq!(*sent.lock().unwrap(), ["attack"]);
    }

    /// A value an automation lane drives stays through a hosted preset; undo puts back the
    /// cleared values with the old state.
    #[test]
    fn automated_values_survive_a_hosted_preset_and_undo_restores_the_rest() {
        let (mut e, a, _, m) = engine();
        let id = soundcraft_model::ModuleId(m as u32);
        let tr = e.session_mut().track_mut(TrackId(a)).unwrap();
        let ins = tr.mixer.module_mut(id).unwrap();
        ins.params.insert("threshold".into(), -30.0);
        ins.params.insert("ratio".into(), 4.0);
        ins.set_state_bytes(b"old");
        let mut lane = soundcraft_model::AutomationLane::new(AutoParam::Plugin { module: id, param: "ratio".into() });
        lane.points.push(soundcraft_model::AutomationPoint::new(0, 4.0));
        tr.automation.push(lane);
        let before = module(&e, a).params.clone();
        e.execute("mix.module_preset", &json!({"track": a, "module": m, "preset": "P", "state": b64::encode(b"new")})).unwrap();
        assert_eq!(module(&e, a).params.keys().collect::<Vec<_>>(), ["ratio"]);
        e.undo();
        assert_eq!(module(&e, a).params, before);
        assert_eq!(module(&e, a).state_bytes().unwrap(), b"old");
    }

    /// A hosted plugin's program (its program parameter) is a step whose undo loads the state
    /// back: the program change lives in the plugin's state.
    #[test]
    fn a_hosted_program_reloads_the_state_on_undo() {
        let (mut e, a, _, m) = engine();
        e.execute("mix.module_state", &json!({"track": a, "module": m, "state": b64::encode(b"live")})).unwrap();
        e.execute("mix.module_preset", &json!({"track": a, "module": m, "preset": "Prog 2", "params": {"2103": 1.0}})).unwrap();
        let ins = module(&e, a);
        assert_eq!((ins.params.get("2103"), ins.preset.as_str()), (Some(&1.0), "Prog 2"));
        let load = ins.state_load;
        assert_ne!(load, 0);
        e.undo();
        let ins = module(&e, a);
        assert!(!ins.params.contains_key("2103"));
        assert_eq!((ins.state_load, ins.state_bytes().unwrap()), (0, b"live".to_vec()), "a new loading of the state from before");
    }

    #[test]
    fn instruments_take_states_or_parameters() {
        let (mut e, _, k, _) = engine();
        // The built-in synth: parameters, clamped; unknown ones refused.
        let info = soundcraft_dsp::plugin_info("subtractive_synth").unwrap();
        let p = &info.params[0];
        let r =
            e.execute("mix.module_preset", &json!({"track": k, "module": "instrument", "preset": "Big", "params": {p.id: p.max + 1000.0}})).unwrap();
        assert_eq!(r["set"], 1);
        let ins = e.session().track(TrackId(k)).unwrap().instrument.clone().unwrap();
        assert_eq!((ins.params[p.id], ins.preset.as_str()), (p.max, "Big"));
        assert!(e.execute("mix.module_preset", &json!({"track": k, "module": "instrument", "preset": "x", "params": {"nope": 1.0}})).is_err());
        assert!(e.execute("mix.module_preset", &json!({"track": k, "module": "instrument", "preset": "x", "state": "AAAA"})).is_err());
        // A hosted instrument: its state.
        e.session_mut().track_mut(TrackId(k)).unwrap().instrument = Some(Insert::new("clap:test.synth"));
        e.execute("mix.module_preset", &json!({"track": k, "module": "instrument", "preset": "Pad", "state": "AAAA"})).unwrap();
        let ins = e.session().track(TrackId(k)).unwrap().instrument.clone().unwrap();
        assert!(ins.state.is_some() && ins.state_load != 0);
    }

    #[test]
    fn bad_requests_change_nothing() {
        let (mut e, a, _, m) = engine();
        let before = e.session().clone();
        for p in [
            json!({"track": a, "module": m, "state": "AAAA"}),
            json!({"track": a, "module": m, "preset": "", "state": "AAAA"}),
            json!({"track": a, "module": m, "preset": "x"}),
            json!({"track": a, "module": m, "preset": "x", "state": "!!"}),
            json!({"track": a, "module": m, "preset": "x", "state": 5}),
            json!({"track": a, "module": m, "preset": "x", "params": {"gain": "loud"}}),
            json!({"track": a, "module": 999, "preset": "x", "state": "AAAA"}),
            json!({"track": a, "module": "instrument", "preset": "x", "state": "AAAA"}),
            json!({"module": m, "preset": "x", "state": "AAAA"}),
        ] {
            assert!(e.execute("mix.module_preset", &p).is_err(), "{p}");
        }
        // A built-in chain module: its presets go through mix.module_params.
        let g = e.execute("mix.module_add", &json!({"track": a, "plugin": "gain"})).unwrap()["module"].as_u64().unwrap();
        let err = e.execute("mix.module_preset", &json!({"track": a, "module": g, "preset": "x", "params": {}})).unwrap_err().to_string();
        assert!(err.contains("mix.module_params"), "{err}");
        assert_eq!(e.session().tracks[0].mixer.inserts[0], before.tracks[0].mixer.inserts[0]);
    }
}
