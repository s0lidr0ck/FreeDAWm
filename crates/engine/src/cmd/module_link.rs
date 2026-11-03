//! `mix.module_link`: puts a module into a session-level instance link group
//! (`Insert::link_group`), for plugins that declare `PluginInfo::link` (FireStation LevelUp's
//! IN/OUT pairs, Shale's Kit Link). Every module in a group, on any track, shares one value bus
//! the mix engine makes when it syncs; the group's producers render before its consumers. A group
//! is just a number: it exists while a module is in it.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::Session;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "mix.module_link",
        "Set Module Link Group",
        [],
        None,
        "{track?, module: id, group: number | null}: puts the module into that instance link group (any track; null: unlinked). The plugin must link (`PluginInfo.link`). Refused when the module, or another linked one, could then not render after its producer in the same block (a routing cycle, a producer later in the same chain, or on an aux or master fader feeding an audio track) → {group, members: [{track, module, role, status}]}",
        has_selection,
        module_link
    )]
}

fn module_link(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.module_link";
    let t = super::module::first_track(e, C, p)?;
    let id = super::module::module_id(C, p)?;
    let group = match p.get("group") {
        None => return Err(bad(C, "`group` required (a link group number, or null for none)")),
        Some(Value::Null) => None,
        Some(v) => {
            Some(v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| bad(C, "`group` must be a whole number (0 to 4294967295) or null"))?)
        }
    };
    let ins = super::module::find_mut(e, C, t, id)?;
    // A plugin this build doesn't know (a missing third-party one) can still be unlinked.
    if group.is_some() && super::clap::plugin_info(&ins.plugin).is_none_or(|i| i.link.is_none()) {
        return Err(bad(C, format!("`{}` does not link with other instances", ins.plugin)));
    }
    if let Some(why) = soundcraft_mix::link_refusal(e.session(), t, id, group) {
        return Err(bad(C, why));
    }
    super::module::find_mut(e, C, t, id)?.link_group = group;
    Ok(json!({"group": group, "members": group.map(|g| members(e.session(), g)).unwrap_or_default()}))
}

/// The modules of link group `g` as the engine resolves them: track, module, role and status.
pub fn members(s: &Session, g: u32) -> Vec<Value> {
    soundcraft_mix::link_routes(s)
        .iter()
        .filter(|r| r.group == g)
        .map(|r| {
            let role = match r.role {
                None => "off",
                Some(soundcraft_dsp::LinkRole::Producer) => "producer",
                Some(soundcraft_dsp::LinkRole::Consumer) => "consumer",
                Some(soundcraft_dsp::LinkRole::Peer) => "peer",
            };
            let status = match r.result {
                Ok(()) => "ok".to_string(),
                Err(p) => p.to_string(),
            };
            json!({"track": r.track, "module": r.module.0, "role": role, "status": status})
        })
        .collect()
}

/// One line per linked module an edit from `before` to `after` made read a block late (all such
/// ones when `before` is `None`). Cheap when no module is linked.
pub(crate) fn link_report(before: Option<&Session>, after: &Session) -> Vec<String> {
    let linked = |s: &Session| s.tracks.iter().any(|t| t.mixer.inserts.iter().any(|i| i.link_group.is_some()));
    if !linked(after) {
        return Vec::new();
    }
    let b = before.filter(|b| linked(b)).map(soundcraft_mix::link_routes).unwrap_or_default();
    soundcraft_mix::link_regressions(&b, &soundcraft_mix::link_routes(after)).iter().map(|r| soundcraft_mix::describe_link(after, r)).collect()
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::{Value, json};
    use soundcraft_model::{ModuleId, TrackId};

    fn probe_track(e: &mut Engine, role: f32) -> (TrackId, u64) {
        e.execute("track.new", &json!({"count": 1, "format": "stereo"})).unwrap();
        let t = e.session().tracks.last().unwrap().id;
        let m =
            e.execute("mix.module_add", &json!({"track": t, "plugin": "test_link", "params": {"role": role}})).unwrap()["module"].as_u64().unwrap();
        (t, m)
    }

    fn group(e: &Engine, t: TrackId, m: u64) -> Option<u32> {
        e.session().track(t).unwrap().mixer.module(ModuleId(m as u32)).unwrap().link_group
    }

    #[test]
    fn module_link_sets_clears_undoes_and_lists_members() {
        let mut e = Engine::new(soundcraft_model::Session::default());
        let (a, ma) = probe_track(&mut e, 2.0);
        let (b, mb) = probe_track(&mut e, 1.0);
        let r = e.execute("mix.module_link", &json!({"track": a, "module": ma, "group": 4})).unwrap();
        assert_eq!(r["group"], 4);
        assert_eq!(r["members"][0]["role"], "consumer");
        assert!(r["members"][0]["status"].as_str().unwrap().contains("no partner"));
        let r = e.execute("mix.module_link", &json!({"track": b, "module": mb, "group": 4})).unwrap();
        let m = r["members"].as_array().unwrap();
        assert_eq!(m.len(), 2);
        assert!(m.iter().all(|x| x["status"] == "ok"), "{r}");
        assert_eq!(group(&e, b, mb), Some(4));
        e.run("edit.undo").unwrap();
        assert_eq!(group(&e, b, mb), None);
        e.run("edit.redo").unwrap();
        assert_eq!(group(&e, b, mb), Some(4));
        e.execute("mix.module_link", &json!({"track": b, "module": mb, "group": null})).unwrap();
        assert_eq!(group(&e, b, mb), None);
        // nonsense
        for g in [json!(-1), json!(1.5), json!("x"), json!(u64::from(u32::MAX) + 1)] {
            assert!(e.execute("mix.module_link", &json!({"track": a, "module": ma, "group": g})).is_err(), "{g}");
        }
        assert!(e.execute("mix.module_link", &json!({"track": a, "module": ma})).is_err());
        assert!(e.execute("mix.module_link", &json!({"track": a, "module": 999, "group": 1})).is_err());
    }

    #[test]
    fn plugins_that_do_not_link_are_refused_but_can_be_unlinked() {
        let mut e = Engine::new(soundcraft_model::Session::default());
        let (t, _) = probe_track(&mut e, 0.0);
        let comp = e.execute("mix.module_add", &json!({"track": t, "plugin": "compressor"})).unwrap()["module"].as_u64().unwrap();
        let err = e.execute("mix.module_link", &json!({"track": t, "module": comp, "group": 1})).unwrap_err().to_string();
        assert!(err.contains("does not link"), "{err}");
        e.execute("mix.module_link", &json!({"track": t, "module": comp, "group": null})).unwrap();
    }

    #[test]
    fn unorderable_links_are_refused_and_edits_that_make_one_are_reported() {
        let mut e = Engine::new(soundcraft_model::Session::default());
        let (t, p) = probe_track(&mut e, 1.0);
        let c = e.execute("mix.module_add", &json!({"track": t, "plugin": "test_link", "params": {"role": 2.0}, "at": 0})).unwrap()["module"]
            .as_u64()
            .unwrap();
        e.execute("mix.module_link", &json!({"track": t, "module": p, "group": 1})).unwrap();
        // the consumer sits before its producer in the chain
        let err = e.execute("mix.module_link", &json!({"track": t, "module": c, "group": 1})).unwrap_err().to_string();
        assert!(err.contains("chain"), "{err}");
        // move it after, link it, then move it back: the move is reported
        e.execute("mix.module_move", &json!({"track": t, "module": c, "to": 1})).unwrap();
        e.execute("mix.module_link", &json!({"track": t, "module": c, "group": 1})).unwrap();
        let r = e.execute("mix.module_move", &json!({"track": t, "module": c, "to": 0})).unwrap();
        let w = r["sidechain_warnings"].as_array().map(Vec::as_slice).unwrap_or_default();
        assert!(w.iter().filter_map(Value::as_str).any(|x| x.contains("link group 1")), "{r}");
        assert_eq!(r["routing_warnings"], r["sidechain_warnings"], "the general key carries the same");
        // a role change that does so (a parameter edit) is reported too
        e.execute("mix.module_move", &json!({"track": t, "module": c, "to": 1})).unwrap();
        let r = e.execute("mix.module_param", &json!({"track": t, "module": p, "param": "role", "value": 2.0})).unwrap();
        assert!(r.get("sidechain_warnings").is_none(), "two consumers, no producer: alone, not late ({r})");
        let r = e.execute("mix.module_param", &json!({"track": t, "module": c, "param": "role", "value": 1.0})).unwrap();
        assert!(r["sidechain_warnings"].as_array().is_some_and(|w| !w.is_empty()), "{r}");
    }

    #[test]
    fn links_cross_tracks_survive_copy_delete_and_a_save() {
        let mut e = Engine::new(soundcraft_model::Session::default());
        let (a, ma) = probe_track(&mut e, 2.0);
        let (b, mb) = probe_track(&mut e, 1.0);
        e.execute("mix.module_link", &json!({"track": a, "module": ma, "group": 2})).unwrap();
        e.execute("mix.module_link", &json!({"track": b, "module": mb, "group": 2})).unwrap();
        // a copy to a third track joins the group
        e.execute("track.new", &json!({"count": 1, "format": "stereo"})).unwrap();
        let c = e.session().tracks.last().unwrap().id;
        let r = e.execute("mix.module_copy", &json!({"track": a, "module": ma, "to_track": c})).unwrap();
        let mc = r["module"].as_u64().unwrap();
        assert_eq!(group(&e, c, mc), Some(2));
        // deleting the producer's track leaves the consumers alone, rendering
        e.execute("track.delete", &json!({"tracks": [b]})).unwrap();
        let routes = soundcraft_mix::link_routes(e.session());
        assert!(routes.iter().all(|r| r.result == Err(soundcraft_mix::LinkProblem::Alone)), "{routes:?}");
        let out = soundcraft_mix::render_range(e.session(), soundcraft_time::Range::new(0, 4096), 512);
        assert!(out.iter().flatten().all(|v| v.is_finite()));
        e.run("edit.undo").unwrap();
        assert!(soundcraft_mix::link_routes(e.session()).iter().all(|r| r.result.is_ok()));
        // save and open
        let dir = std::env::temp_dir().join(format!("sc-module-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.scraft");
        crate::io::save_session(&mut e, &path.display().to_string()).unwrap();
        let mut f = Engine::new(soundcraft_model::Session::default());
        crate::io::open_session(&mut f, &path.display().to_string()).unwrap();
        assert_eq!(group(&f, a, ma), Some(2));
        assert_eq!(group(&f, b, mb), Some(2));
        let _ = std::fs::remove_dir_all(dir);
    }
}
