//! Restarting a hosted plug-in (`mix.module_restart`): its running instance is made again from
//! the state the session holds for it. For a plug-in run in its own process that crashed or hung
//! (it is silenced, an effect passing its input through), this brings it back in a new process,
//! with its last state and the parameter values the session holds; a live one just takes that
//! state again.

use super::*;
use serde_json::json;
use soundcraft_model::Insert;

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: "mix.module_restart",
        label: "Restart Plug-in",
        menu: &[],
        shortcut: None,
        params: "{track, module: id | \"instrument\", state?: base64 (the plug-in's last state, which the app reads from the running instance first)} → {bytes}. A hosted plug-in's running instance is made again from the stored state and parameter values. Not undoable (nothing in the session changes but the state it already held) and not journaled.",
        enabled: has_tracks,
        run: module_restart,
        journal: false,
        undoable: false,
        view: false,
    }]
}

const C: &str = "mix.module_restart";

fn module_restart(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = track_param(e, C, p, "track")?.ok_or_else(|| bad(C, "`track` required"))?;
    let state = match p.get("state") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            Some(soundcraft_model::b64::decode(s).filter(|b| !b.is_empty()).ok_or_else(|| bad(C, "`state` is not valid base64"))?)
        }
        Some(_) => return Err(bad(C, "`state` must be a base64 string")),
    };
    let instrument = p.get("module").and_then(Value::as_str).is_some_and(|s| s.trim().eq_ignore_ascii_case("instrument"));
    let id = if instrument { None } else { Some(super::module::module_id(C, p)?) };
    let tr = e.session_mut().track_mut(t).ok_or_else(|| bad(C, "no track"))?;
    let ins: &mut Insert = match id {
        None => tr.instrument.as_mut().ok_or_else(|| bad(C, "the track has no instrument"))?,
        Some(m) => tr.mixer.module_mut(m).ok_or_else(|| bad(C, "no such module"))?,
    };
    if !soundcraft_mix::is_third_party(&ins.plugin) {
        return Err(bad(C, "only a hosted plug-in is restarted (built-in modules run in FreeDAWm itself)"));
    }
    if let Some(bytes) = state {
        ins.set_state_bytes(&bytes);
    }
    // A new loading of the stored state: the player makes the instance again from it.
    ins.state_load = super::module_preset::next_load();
    Ok(json!({"bytes": ins.state_bytes().map_or(0, |b| b.len())}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::{Insert, TrackId, b64};

    #[test]
    fn a_restart_loads_the_stored_state_again_and_keeps_the_values() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"name": "A"})).unwrap();
        let a = e.session().tracks[0].id;
        let mut ins = Insert::new("clap:org.example.crashy");
        ins.params.insert("7".into(), 0.5);
        let m = e.session_mut().track_mut(a).unwrap().mixer.add_module(ins, None).unwrap();
        let module = |e: &Engine| e.session().track(TrackId(a.0)).unwrap().mixer.module(m).cloned().unwrap();
        let before = module(&e).state_load;
        let undo = e.undo_label().map(str::to_string);
        let r = e.execute("mix.module_restart", &json!({"track": a.0, "module": m.0, "state": b64::encode(b"last")})).unwrap();
        assert_eq!(r["bytes"], json!(4));
        let after = module(&e);
        assert_ne!(after.state_load, before, "the instance is made again");
        assert_eq!(after.state_bytes().as_deref(), Some(&b"last"[..]), "from its last state");
        assert_eq!(after.params.get("7"), Some(&0.5), "parameter values stay");
        assert_eq!(e.undo_label().map(str::to_string), undo, "not an undo step");
        // Again without a state: the stored one is loaded once more.
        e.execute("mix.module_restart", &json!({"track": a.0, "module": m.0})).unwrap();
        assert_ne!(module(&e).state_load, after.state_load);
        assert_eq!(module(&e).state_bytes().as_deref(), Some(&b"last"[..]));
        // Built-in modules and missing ones are refused.
        e.execute("mix.insert", &json!({"track": a.0, "plugin": "eq_7band"})).unwrap();
        let eq = e.session().tracks[0].mixer.inserts[1].id;
        assert!(e.execute("mix.module_restart", &json!({"track": a.0, "module": eq.0})).is_err());
        assert!(e.execute("mix.module_restart", &json!({"track": a.0, "module": 999})).is_err());
        assert!(e.execute("mix.module_restart", &json!({"track": a.0, "module": "instrument"})).is_err());
    }
}
