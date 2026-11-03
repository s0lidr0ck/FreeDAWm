//! FireStation CleanQ's ceiling (`mix.module_ceiling`): Learn, Draw and Clear, as the C++ editor's
//! Ceiling panel does them (`PluginEditor.cpp`: Auto Generate, dragging a node, a cleared shape).
//!
//! The shape lives in the module's state (`Insert::state`): the C++ `uiState/ceiling` element
//! (`CeilingState`), so its other settings (the noise guides, the averaging, the snap) ride along
//! untouched. Every change also makes the shape ready for the audio thread here, off it
//! (`Insert::prepared`: the 256-point grid), which the mix engine copies into the running plugin
//! between blocks. Opening a session does the same for every module ([`prepare_states`]).

use super::module::{find_mut, first_track, module_id};
use super::*;
use crate::cmd;
use serde_json::json;
// (The FireStation engines' module, named without its literal path: `file_access` keeps any
// `fs` + `::` out of the engine's code.)
use firestation::cleanq::clean_q_ceiling::{self as cl, Ceiling, GRID_N, NUM_NODES, Point};
use firestation::cleanq::clean_q_ceiling_state::CeilingState;
use firestation::cleanq::clean_q_guide_templates::{self as templates, TEMPLATES};
use firestation::cleanq::clean_q_guides::Guides;
use soundcraft_dsp::plugins::fs as firestation;
use soundcraft_model::{Insert, PreparedState, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.module_ceiling",
            "Set CleanQ Ceiling",
            [],
            None,
            "{track?, module: id (a FireStation CleanQ), one of: learn: [256 numbers] (the averaged capture of the input on the ceiling grid, dB: Learn lays the 16 nodes on it, where it turns) | points: [[hz, db] × 16] (Draw: the nodes, limited to 20 Hz..20 kHz and -120..0 dB, sorted) | points: null (Clear)} → {points: [[hz, db]] | null}. Undoable; the dynamic ceiling (ceilOn) cuts against it.",
            has_selection,
            module_ceiling
        ),
        cmd!(
            "mix.module_guides",
            "Set CleanQ Guides",
            [],
            None,
            "{track?, module: id (a FireStation CleanQ), one of: guides: [{freq, lo?, hi?, gain?, dynamic?, done?, note?, area?, hint?}] | null (none) | template: id or name (a source's typical problem areas: male_vocal, female_vocal, kick_in, kick_out, snare_top, snare_bottom, rack_tom, floor_tom, overheads, hi_hat, room_mic, acoustic_guitar, electric_guitar_rhythm, lead_guitar, bass_guitar, piano) with replace?: bool (default false: added to the guides there, skipping ones already there)} → {guides: n, added?, skipped?, full?}. CleanQ's guides (`uiState/guides`: notes pinned to a region of the spectrum, no audio), kept in the module's state next to the ceiling; at most 24, each sanitised as the plugin does. `area` (the area's name) and `hint` (the usual move) are optional texts a template fills; a template's guides propose no gain and set no band. Undoable.",
            has_selection,
            module_guides
        ),
    ]
}

/// `mix.module_guides`: the guides replaced, the rest of the stored state kept.
fn module_guides(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.module_guides";
    let t = first_track(e, C, p)?;
    let id = module_id(C, p)?;
    // A template (added to the guides there, or replacing them), or the list itself.
    let template = match p.get("template") {
        None | Some(Value::Null) => None,
        Some(Value::String(k)) => Some(templates::template(k).ok_or_else(|| {
            let ids: Vec<&str> = TEMPLATES.iter().map(|t| t.id).collect();
            bad(C, format!("no guide template `{k}` (one of: {})", ids.join(", ")))
        })?),
        Some(_) => return Err(bad(C, "`template` must be a template's id or name")),
    };
    let replace = match p.get("replace") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(C, "`replace` must be true or false")),
    };
    let guides = match (template, p.get("guides")) {
        (Some(_), Some(g)) if !g.is_null() => return Err(bad(C, "give `guides` or `template`, not both")),
        (Some(_), _) => None,
        (None, None) => return Err(bad(C, "`guides` required (an array, or null for none), or a `template`")),
        (None, Some(Value::Null)) => Some(Guides::default()),
        (None, Some(v @ Value::Array(_))) => Some(Guides::from_json(v)),
        (None, Some(_)) => return Err(bad(C, "`guides` must be an array of guides, or null")),
    };
    let ins = find_mut(e, C, t, id)?;
    if ins.plugin != "fs_cleanq" {
        return Err(bad(C, format!("`{}` has no guides (a FireStation CleanQ does)", ins.plugin)));
    }
    let mut st = ins.state_bytes().and_then(|b| CeilingState::read(&b)).unwrap_or_default();
    let mut out = json!({});
    if let Some(g) = guides {
        st.guides = g;
    } else if let Some(tpl) = template {
        let a = templates::apply(&mut st.guides, tpl, replace);
        out = json!({"added": a.added, "skipped": a.skipped, "full": a.full});
    }
    ins.set_state_bytes(&st.write());
    prepare_insert(ins);
    out["guides"] = json!(st.guides.list.len());
    Ok(out)
}

/// Makes a module's stored state ready for its plugin (`Insert::prepared`), or clears that when
/// the plugin keeps no such state or the module has none.
pub(crate) fn prepare_insert(ins: &mut Insert) {
    ins.prepared = ins.state_bytes().and_then(|b| soundcraft_dsp::prepare_state(&ins.plugin, &b)).map(PreparedState::new);
}

/// [`prepare_insert`] for every module and instrument of a session (one just opened).
pub(crate) fn prepare_states(s: &mut Session) {
    for t in &mut s.tracks {
        for ins in &mut t.mixer.inserts {
            prepare_insert(ins);
        }
        if let Some(ins) = t.instrument.as_mut() {
            prepare_insert(ins);
        }
    }
}

/// The nodes as `[[hz, db], …]`, or null when there is no shape.
fn points_json(c: &Ceiling) -> Value {
    if c.pts.is_empty() { Value::Null } else { json!(c.pts.iter().map(|p| [p.hz, p.db]).collect::<Vec<_>>()) }
}

/// Learn: Auto Generate on a capture (`onAutoGenerate`), with the editor's messages.
fn learned(cmd: &str, v: &Value) -> Result<Ceiling> {
    let arr = v.as_array().ok_or_else(|| bad(cmd, "`learn` must be an array of numbers"))?;
    if arr.is_empty() {
        return Err(bad(cmd, "Nothing captured yet: play something first."));
    }
    if arr.len() != GRID_N {
        return Err(bad(cmd, format!("`learn` must hold {GRID_N} values, one per ceiling grid point")));
    }
    let mut grid = Vec::with_capacity(GRID_N);
    for x in arr {
        match x.as_f64() {
            Some(d) if d.is_finite() => grid.push(d),
            _ => return Err(bad(cmd, "`learn` values must be finite numbers")),
        }
    }
    let c = cl::auto_ceiling(&grid);
    if !c.is_valid() {
        return Err(bad(cmd, "Not enough signal to generate from."));
    }
    Ok(c)
}

/// Draw: the 16 nodes, read as the state reads them (`jlimit`, then sorted).
fn drawn(cmd: &str, v: &Value) -> Result<Ceiling> {
    let arr = v.as_array().ok_or_else(|| bad(cmd, "`points` must be an array of [hz, db] pairs, or null"))?;
    if arr.len() != NUM_NODES {
        return Err(bad(cmd, format!("a ceiling is always {NUM_NODES} nodes")));
    }
    let mut c = Ceiling::default();
    for p in arr {
        let pair = p.as_array().filter(|a| a.len() == 2).ok_or_else(|| bad(cmd, "each point is [hz, db]"))?;
        let (Some(hz), Some(db)) = (pair[0].as_f64(), pair[1].as_f64()) else { return Err(bad(cmd, "each point is [hz, db]")) };
        if !hz.is_finite() || !db.is_finite() {
            return Err(bad(cmd, "points must be finite"));
        }
        c.pts.push(Point { hz: hz.clamp(cl::GRID_LO, cl::GRID_HI), db: db.clamp(-120.0, 0.0) });
    }
    c.sort();
    Ok(c)
}

fn module_ceiling(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.module_ceiling";
    let t = first_track(e, C, p)?;
    let id = module_id(C, p)?;
    let shape = match (p.get("learn"), p.get("points")) {
        (Some(_), Some(_)) => return Err(bad(C, "give `learn` or `points`, not both")),
        (Some(l), None) => learned(C, l)?,
        (None, Some(Value::Null)) => Ceiling::default(),
        (None, Some(v)) => drawn(C, v)?,
        (None, None) => return Err(bad(C, "`learn` or `points` required")),
    };
    let ins = find_mut(e, C, t, id)?;
    if ins.plugin != "fs_cleanq" {
        return Err(bad(C, format!("`{}` has no ceiling (a FireStation CleanQ does)", ins.plugin)));
    }
    // The rest of the panel's state is kept; a corrupt stored state starts from the defaults.
    let mut st = ins.state_bytes().and_then(|b| CeilingState::read(&b)).unwrap_or_default();
    st.shape = shape;
    ins.set_state_bytes(&st.write());
    prepare_insert(ins);
    Ok(json!({"points": points_json(&st.shape)}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, TrackKind};

    fn engine() -> (Engine, u64, u32) {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Vox"));
        let r = e.execute("mix.module_add", &json!({"track": t.0, "plugin": "fs_cleanq"})).unwrap();
        let m = r["module"].as_u64().unwrap() as u32;
        (e, t.0, m)
    }

    fn ins(e: &Engine) -> &Insert {
        &e.session().tracks[0].mixer.inserts[0]
    }

    fn capture() -> Vec<f64> {
        (0..GRID_N)
            .map(|i| -40.0 + 9.0 * (-((cl::octaves_from_1k(cl::grid_hz(i)) - 1.5).powi(2)) * 6.0).exp() - 2.0 * cl::octaves_from_1k(cl::grid_hz(i)))
            .collect()
    }

    #[test]
    fn guides_are_set_undoable_and_keep_the_ceiling() {
        let (mut e, t, m) = engine();
        e.execute("mix.module_ceiling", &json!({"track": t, "module": m, "learn": capture()})).unwrap();
        let shape = CeilingState::read(&ins(&e).state_bytes().unwrap()).unwrap().shape;
        let r = e
            .execute(
                "mix.module_guides",
                &json!({"track": t, "module": m, "guides": [{"freq": 300, "note": "mud"}, {"freq": 0}, {"freq": 5000, "gain": 2, "done": true}]}),
            )
            .unwrap();
        assert_eq!(r["guides"], 2);
        assert_eq!(e.undo_label(), Some("Set CleanQ Guides"));
        let st = CeilingState::read(&ins(&e).state_bytes().unwrap()).unwrap();
        assert_eq!(st.shape, shape, "the ceiling is kept");
        assert_eq!((st.guides.list[0].note.as_str(), st.guides.list[1].done), ("mud", true));
        let many: Vec<Value> = (0..100).map(|i| json!({"freq": 100 + i})).collect();
        e.execute("mix.module_guides", &json!({"track": t, "module": m, "guides": many})).unwrap();
        assert_eq!(CeilingState::read(&ins(&e).state_bytes().unwrap()).unwrap().guides.list.len(), 24);
        e.undo();
        assert_eq!(CeilingState::read(&ins(&e).state_bytes().unwrap()).unwrap().guides.list.len(), 2);
        e.execute("mix.module_guides", &json!({"track": t, "module": m, "guides": null})).unwrap();
        assert!(CeilingState::read(&ins(&e).state_bytes().unwrap()).unwrap().guides.list.is_empty());
        assert!(e.execute("mix.module_guides", &json!({"track": t, "module": m})).is_err());
        assert!(e.execute("mix.module_guides", &json!({"track": t, "module": m, "guides": 3})).is_err());
    }

    #[test]
    fn a_template_adds_its_guides_undoably() {
        let (mut e, t, m) = engine();
        let guides = |e: &Engine| CeilingState::read(&ins(e).state_bytes().unwrap_or_default()).unwrap_or_default().guides;
        e.execute("mix.module_guides", &json!({"track": t, "module": m, "guides": [{"freq": 452, "note": "mine"}]})).unwrap();
        let params_before = ins(&e).params.clone();
        let r = e.execute("mix.module_guides", &json!({"track": t, "module": m, "template": "Female vocal"})).unwrap();
        assert_eq!((r["guides"].as_u64(), r["added"].as_u64(), r["skipped"].as_u64()), (Some(7), Some(6), Some(1)), "{r}");
        assert_eq!(e.undo_label(), Some("Set CleanQ Guides"));
        let g = guides(&e);
        assert_eq!(g.list[0].note, "mine", "yours are kept");
        let prox = &g.list[2];
        assert_eq!((prox.area.as_str(), prox.freq_hz, prox.lo_hz, prox.hi_hz, prox.dynamic), ("Proximity warmth", 250.0, 180.0, 350.0, true));
        assert_eq!((prox.note.as_str(), prox.hint.as_str(), prox.gain_db), ("thick, woofy", "cut 2\u{2013}3 dB, Dyn", 0.0));
        assert_eq!(ins(&e).params, params_before, "a template sets no band, no gain");
        // Replace, then undo back to the added list, then to yours alone.
        let r = e.execute("mix.module_guides", &json!({"track": t, "module": m, "template": "kick_in", "replace": true})).unwrap();
        assert_eq!((r["guides"].as_u64(), r["added"].as_u64()), (Some(5), Some(5)));
        assert_eq!(guides(&e).list[0].area, "Sub thump");
        e.undo();
        assert_eq!(guides(&e).list.len(), 7);
        e.undo();
        assert_eq!(guides(&e).list.len(), 1);
        // Bad requests.
        assert!(e.execute("mix.module_guides", &json!({"track": t, "module": m, "template": "kazoo"})).is_err());
        assert!(e.execute("mix.module_guides", &json!({"track": t, "module": m, "template": 3})).is_err());
        assert!(e.execute("mix.module_guides", &json!({"track": t, "module": m, "template": "piano", "replace": "yes"})).is_err());
        assert!(e.execute("mix.module_guides", &json!({"track": t, "module": m, "template": "piano", "guides": []})).is_err());
    }

    #[test]
    fn learn_draw_clear_are_undoable_and_prepared() {
        let (mut e, t, m) = engine();
        assert!(ins(&e).state.is_none() && ins(&e).prepared.is_none());

        // Learn: 16 nodes on the capture, the state written and the grid made ready.
        let r = e.execute("mix.module_ceiling", &json!({"track": t, "module": m, "learn": capture()})).unwrap();
        let want = cl::auto_ceiling(&capture());
        assert_eq!(r["points"].as_array().unwrap().len(), NUM_NODES);
        let st = CeilingState::read(&ins(&e).state_bytes().unwrap()).unwrap();
        assert_eq!(st.shape, want);
        assert_eq!(&*ins(&e).prepared.as_ref().unwrap().data, want.to_grid().as_slice());
        assert_eq!(e.undo_label(), Some("Set CleanQ Ceiling"));

        // Draw: move node 4 down, every node sent; the panel's other settings are kept.
        let mut c = want.clone();
        let db = c.pts[4].db - 5.0;
        cl::move_node(&mut c, 4, db, true);
        let pts: Vec<[f64; 2]> = c.pts.iter().map(|p| [p.hz, p.db]).collect();
        e.execute("mix.module_ceiling", &json!({"track": t, "module": m, "points": pts})).unwrap();
        let st = CeilingState::read(&ins(&e).state_bytes().unwrap()).unwrap();
        assert_eq!(st.shape, c);
        assert_eq!(&*ins(&e).prepared.as_ref().unwrap().data, c.to_grid().as_slice());

        // Clear.
        let r = e.execute("mix.module_ceiling", &json!({"track": t, "module": m, "points": null})).unwrap();
        assert!(r["points"].is_null());
        assert!(CeilingState::read(&ins(&e).state_bytes().unwrap()).unwrap().shape.pts.is_empty());
        assert!(ins(&e).prepared.as_ref().unwrap().data.is_empty(), "prepared as no ceiling");

        // Undo walks back through Draw and Learn; redo forward again.
        assert!(e.undo());
        assert_eq!(&*ins(&e).prepared.as_ref().unwrap().data, c.to_grid().as_slice());
        assert!(e.undo());
        assert_eq!(&*ins(&e).prepared.as_ref().unwrap().data, want.to_grid().as_slice());
        assert!(e.undo());
        assert!(ins(&e).state.is_none() && ins(&e).prepared.is_none());
        assert!(e.redo());
        assert_eq!(CeilingState::read(&ins(&e).state_bytes().unwrap()).unwrap().shape, want);
    }

    #[test]
    fn a_drag_is_one_undo_step() {
        let (mut e, t, m) = engine();
        e.execute("mix.module_ceiling", &json!({"track": t, "module": m, "learn": capture()})).unwrap();
        let steps = e.undo_history().len();
        let base = cl::auto_ceiling(&capture());
        for k in 1..=10 {
            let mut c = base.clone();
            let db = c.pts[7].db - k as f64;
            cl::move_node(&mut c, 7, db, true);
            let pts: Vec<[f64; 2]> = c.pts.iter().map(|p| [p.hz, p.db]).collect();
            e.execute_merged("mix.module_ceiling", &json!({"track": t, "module": m, "points": pts}), "ceiling-drag").unwrap();
        }
        e.end_merge();
        assert_eq!(e.undo_history().len(), steps + 1);
        assert!(e.undo());
        assert_eq!(CeilingState::read(&ins(&e).state_bytes().unwrap()).unwrap().shape, base);
    }

    #[test]
    fn bad_inputs_are_clean_errors() {
        let (mut e, t, m) = engine();
        let before = e.session_arc();
        let flat: Vec<[f64; 2]> = cl::flat_ceiling(-30.0).pts.iter().map(|p| [p.hz, p.db]).collect();
        for p in [
            json!({"track": t, "module": m}),
            json!({"track": t, "module": m, "learn": []}),
            json!({"track": t, "module": m, "learn": [1.0, 2.0]}),
            json!({"track": t, "module": m, "learn": "x"}),
            json!({"track": t, "module": m, "learn": vec![-40.0; GRID_N - 1].into_iter().chain([f64::MAX]).map(|v| if v == f64::MAX { json!("nan") } else { json!(v) }).collect::<Vec<_>>()}),
            json!({"track": t, "module": m, "points": [[100.0, -3.0]]}),
            json!({"track": t, "module": m, "points": vec![[1.0, 2.0, 3.0]; 16]}),
            json!({"track": t, "module": m, "points": "garbage"}),
            json!({"track": t, "module": m, "points": flat, "learn": capture()}),
            json!({"track": t, "module": 999, "points": null}),
        ] {
            assert!(e.execute("mix.module_ceiling", &p).is_err(), "{p}");
        }
        assert!(std::sync::Arc::ptr_eq(&before, &e.session_arc()), "nothing changed");
        // Not a CleanQ.
        let r = e.execute("mix.module_add", &json!({"track": t, "plugin": "fs_comp"})).unwrap();
        assert!(e.execute("mix.module_ceiling", &json!({"track": t, "module": r["module"], "points": null})).is_err());
        // Out-of-range nodes are limited, as the state reads them.
        let wild: Vec<[f64; 2]> = (0..16).map(|k| [1.0e9 - k as f64, 50.0]).collect();
        e.execute("mix.module_ceiling", &json!({"track": t, "module": m, "points": wild})).unwrap();
        let st = CeilingState::read(&ins(&e).state_bytes().unwrap()).unwrap();
        assert!(st.shape.pts.iter().all(|p| p.hz == cl::GRID_HI && p.db == 0.0));
        // A corrupt stored state: a new shape starts from the defaults, nothing panics.
        let mut s = (*e.session_arc()).clone();
        s.tracks[0].mixer.inserts[0].set_state_bytes(&[0xff, 0xfe, 0x00]);
        e.replace_session(s);
        assert_eq!(ins(&e).prepared.as_ref().map(|p| p.data.len()), Some(0), "a corrupt state is prepared as no ceiling");
        e.execute("mix.module_ceiling", &json!({"track": t, "module": m, "points": flat})).unwrap();
        assert_eq!(CeilingState::read(&ins(&e).state_bytes().unwrap()).unwrap().shape, cl::flat_ceiling(-30.0));
    }

    #[test]
    fn a_saved_ceiling_comes_back_prepared() {
        let (mut e, t, m) = engine();
        e.execute("mix.module_ceiling", &json!({"track": t, "module": m, "learn": capture()})).unwrap();
        let json = serde_json::to_string(e.session()).unwrap();
        let mut s: Session = serde_json::from_str(&json).unwrap();
        assert!(s.tracks[0].mixer.inserts[0].prepared.is_none(), "not saved");
        s.tracks[0].mixer.inserts[0].prepared = None;
        let mut f = Engine::default();
        f.replace_session(s);
        assert_eq!(&*ins(&f).prepared.as_ref().unwrap().data, cl::auto_ceiling(&capture()).to_grid().as_slice());
        // mix.module_state on a built-in prepares too.
        let state = soundcraft_model::b64::encode(&CeilingState { shape: cl::flat_ceiling(-20.0), ..CeilingState::default() }.write());
        f.execute("mix.module_state", &json!({"track": t, "module": m, "state": state})).unwrap();
        assert_eq!(&*ins(&f).prepared.as_ref().unwrap().data, cl::flat_ceiling(-20.0).to_grid().as_slice());
    }
}
