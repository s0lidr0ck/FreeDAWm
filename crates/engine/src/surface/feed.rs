//! The mixer as a feed for a controlling PC (the network's `mixer.subscribe`): a compact JSON
//! picture of every strip, built from the session the way the control surfaces read it (what a
//! fader shows follows its automation while it reads: [`Engine::automation_shown`]), and the
//! difference between two pictures, so a client gets one snapshot and then only what changed.
//!
//! The picture is a plain [`Value`] so the network side can diff and send it without knowing the
//! model. Built on the UI thread (a few hundred microseconds for 32 strips); never on the audio
//! thread. A later "network surface" reads and writes the same strips as [`super::SurfaceGroup`]
//! does: moves go through the ordinary commands (`mix.volume`, `mix.pan`, `mix.mute`, …) and a
//! drag is one undo step through `engine.execute {gesture}` (see `docs/control-protocol.md`).
//!
//! Third-party (hosted) plug-ins appear by name and bypass only; built-in modules carry their
//! parameter values in plain units (dB, Hz, ms, …: what `mix.module_param` takes).

use crate::Engine;
use serde_json::{Map, Value, json};
use soundcraft_model::{AutoParam, Track, TrackKind};

/// A track's colour as `#rrggbb`.
fn colour(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// Rounds to 0.01 (dB, pan): small float noise from automation never shows up as a change.
fn r2(v: f32) -> Value {
    if v.is_finite() { json!(((f64::from(v)) * 100.0).round() / 100.0) } else { Value::Null }
}

fn insert_json(i: usize, ins: &soundcraft_model::Insert) -> Value {
    let info = crate::cmd::plugin_info(&ins.plugin);
    let hosted = soundcraft_mix::is_third_party(&ins.plugin);
    let mut v = json!({
        "slot": i,
        "module": ins.id,
        "plugin": ins.plugin,
        "name": info.map_or_else(|| ins.plugin.clone(), |p| p.name.to_string()),
        "bypass": ins.bypass,
        "hosted": hosted,
    });
    if !hosted && let Some(info) = info {
        let mut params = Map::new();
        for p in info.params {
            let value = ins.params.get(p.id).copied().unwrap_or(p.default);
            params.insert(p.id.to_string(), r2(value));
        }
        v["params"] = Value::Object(params);
    }
    v
}

fn strip_json(e: &Engine, t: &Track) -> Value {
    let s = e.session();
    let m = &t.mixer;
    let volume = e.automation_shown(t.id, &AutoParam::Volume).unwrap_or(m.volume_db);
    let pan: Vec<Value> = m.pan.iter().enumerate().map(|(k, p)| r2(e.automation_shown(t.id, &AutoParam::Pan(k as u8)).unwrap_or(*p))).collect();
    let groups: Vec<u64> = s.groups.iter().filter(|g| g.members.contains(&t.id)).map(|g| g.id.0).collect();
    let sends: Vec<Value> = m
        .sends
        .iter()
        .enumerate()
        .filter_map(|(i, x)| {
            x.as_ref().map(
                |x| json!({"slot": i, "target": x.target, "level_db": r2(x.level_db), "pan": r2(x.pan), "mute": x.mute, "pre_fader": x.pre_fader}),
            )
        })
        .collect();
    json!({
        "id": t.id,
        "name": t.name,
        "kind": t.kind.id(),
        "color": colour(t.color),
        "format": t.format.label(),
        "volume_db": r2(volume),
        "pan": pan,
        "mute": m.mute,
        "solo": m.solo,
        "record_arm": m.record_arm,
        "input_monitor": m.input_monitor,
        "input": m.input,
        "output": m.output,
        "automation_mode": m.automation_mode.label(),
        "vca": m.vca,
        "groups": groups,
        "inactive": t.inactive,
        "hidden": t.hidden,
        "sends": sends,
        "inserts": m.inserts.iter().enumerate().map(|(i, x)| insert_json(i, x)).collect::<Vec<_>>(),
    })
}

/// The strips a mixer feed shows, in mixer order: every track with a fader (the master, aux and
/// VCA tracks included), never video tracks.
pub fn feed_tracks(s: &soundcraft_model::Session) -> impl Iterator<Item = &Track> {
    s.tracks.iter().filter(|t| t.kind != TrackKind::Video)
}

/// The whole mixer as a feed snapshot (see the module docs).
pub fn mixer_state(e: &Engine) -> Value {
    let s = e.session();
    let strips: Vec<Value> = feed_tracks(s).map(|t| strip_json(e, t)).collect();
    let state = if e.transport.recording {
        "recording"
    } else if e.transport.playing {
        "playing"
    } else {
        "stopped"
    };
    json!({
        "session": s.name,
        "pre_fader_metering": s.edit.pre_fader_metering,
        "solo_mode": s.edit.solo_mode,
        "order": feed_tracks(s).map(|t| t.id).collect::<Vec<_>>(),
        "strips": strips,
        "busses": s.busses.iter().map(|b| json!({"id": b.id, "name": b.name, "format": b.format.label()})).collect::<Vec<_>>(),
        "groups": s.groups.iter().map(|g| json!({"id": g.id, "name": g.name, "letter": g.letter, "members": g.members, "active": g.active, "color": colour(g.color)})).collect::<Vec<_>>(),
        "transport": {"state": state, "loop": s.edit.loop_playback},
        // The next Undo and Redo, by label (a remote's undo buttons act on this history).
        "undo": e.undo_label(),
        "redo": e.redo_label(),
        // The session's mix snapshots (a remote recalls them by index).
        "snapshots": s.mix_snapshots.iter().enumerate().map(|(i, m)| json!({"index": i, "name": m.name})).collect::<Vec<_>>(),
    })
}

/// What changed from `old` to `new` (two [`mixer_state`]s): `None` when nothing did, else an
/// object with `strips` (each changed strip's `id` and only its changed fields; a new strip
/// whole), `removed` (ids of strips gone) and any other top-level field that changed, whole.
pub fn mixer_delta(old: &Value, new: &Value) -> Option<Value> {
    let mut out = Map::new();
    let (Some(o), Some(n)) = (old.as_object(), new.as_object()) else {
        return Some(new.clone());
    };
    for (k, v) in n {
        if k == "strips" {
            continue;
        }
        if o.get(k) != Some(v) {
            out.insert(k.clone(), v.clone());
        }
    }
    let empty = Vec::new();
    let old_strips = o.get("strips").and_then(Value::as_array).unwrap_or(&empty);
    let new_strips = n.get("strips").and_then(Value::as_array).unwrap_or(&empty);
    let id = |v: &Value| v.get("id").and_then(Value::as_u64);
    let mut changed = Vec::new();
    for ns in new_strips {
        let before = old_strips.iter().find(|os| id(os) == id(ns));
        match (before.and_then(Value::as_object), ns.as_object()) {
            (Some(b), Some(a)) => {
                let mut d = Map::new();
                for (k, v) in a {
                    if b.get(k) != Some(v) {
                        d.insert(k.clone(), v.clone());
                    }
                }
                if !d.is_empty() {
                    d.insert("id".into(), json!(id(ns)));
                    changed.push(Value::Object(d));
                }
            }
            _ => changed.push(ns.clone()),
        }
    }
    let removed: Vec<u64> = old_strips.iter().filter_map(id).filter(|i| !new_strips.iter().any(|ns| id(ns) == Some(*i))).collect();
    if !changed.is_empty() {
        out.insert("strips".into(), Value::Array(changed));
    }
    if !removed.is_empty() {
        out.insert("removed".into(), json!(removed));
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

/// [`mixer_delta`] applied: `state` brought up to date (a client's mirror does this; the tests
/// check the feed with it).
pub fn apply_delta(state: &mut Value, delta: &Value) {
    let Some(d) = delta.as_object() else { return };
    let Some(st) = state.as_object_mut() else { return };
    for (k, v) in d {
        if k != "strips" && k != "removed" && k != "seq" {
            st.insert(k.clone(), v.clone());
        }
    }
    let Some(strips) = st.get_mut("strips").and_then(Value::as_array_mut) else { return };
    if let Some(gone) = d.get("removed").and_then(Value::as_array) {
        strips.retain(|s| !gone.contains(s.get("id").unwrap_or(&Value::Null)));
    }
    for c in d.get("strips").and_then(Value::as_array).into_iter().flatten() {
        let cid = c.get("id");
        match strips.iter_mut().find(|s| s.get("id") == cid).and_then(Value::as_object_mut) {
            Some(s) => {
                if let Some(c) = c.as_object() {
                    for (k, v) in c {
                        s.insert(k.clone(), v.clone());
                    }
                }
            }
            None => strips.push(c.clone()),
        }
    }
    // Mixer order as the snapshot gives it.
    if let Some(order) = st.get("order").and_then(Value::as_array).cloned()
        && let Some(strips) = st.get_mut("strips").and_then(Value::as_array_mut)
    {
        strips.sort_by_key(|s| order.iter().position(|o| Some(o) == s.get("id")).unwrap_or(usize::MAX));
    }
}

/// One strip's meters as the Mix window shows them: peak and RMS of its first two channels and
/// the first dynamics insert's gain reduction (pre- or post-fader as the session's Pre-Fader
/// Metering says), linear (gain reduction in dB).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct StripLevels {
    pub peak: [f32; 2],
    pub rms: [f32; 2],
    pub gr_db: f32,
}

/// A level as one byte: half-dB steps from -100 dB (0: -100 dB or quieter, silence) to +27.5 dB.
fn level_byte(lin: f32) -> u8 {
    if !lin.is_finite() || lin <= 1e-5 {
        return 0;
    }
    ((20.0 * lin.log10() + 100.0) * 2.0).round().clamp(0.0, 255.0) as u8
}

/// Standard base64 (with padding).
fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(T.get(((n >> (18 - 6 * i)) & 63) as usize).copied().unwrap_or(b'A')));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The meters message's `params`: `{pos, ids, lv}`. `pos` is the playhead (s); `ids` the strips
/// in order; `lv` base64 of five bytes per strip in that order: peak L, peak R, RMS L, RMS R
/// ([`level_byte`]: dB = byte / 2 - 100, 0 is silence) and gain reduction (quarter dB, 0 none).
/// About 7 bytes a strip.
pub fn meters_json(pos: f64, levels: &[(u64, StripLevels)]) -> Value {
    let mut bytes = Vec::with_capacity(levels.len() * 5);
    for (_, l) in levels {
        bytes.extend([level_byte(l.peak[0]), level_byte(l.peak[1]), level_byte(l.rms[0]), level_byte(l.rms[1])]);
        bytes.push(if l.gr_db.is_finite() { (l.gr_db.abs() * 4.0).round().clamp(0.0, 255.0) as u8 } else { 0 });
    }
    json!({"pos": (pos * 1000.0).round() / 1000.0, "ids": levels.iter().map(|(id, _)| *id).collect::<Vec<_>>(), "lv": base64(&bytes)})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> (Engine, u64, u64) {
        let mut e = crate::demo::demo_engine();
        // Automation off: the faders show their own values, not their lanes'.
        let all: Vec<u64> = e.session().tracks.iter().map(|t| t.id.0).collect();
        e.execute("mix.automation_mode", &json!({"tracks": all, "mode": "off"})).unwrap();
        let id = |n: &str| e.session().track_by_name(n).unwrap().id.0;
        let (a, b) = (id("Kick"), id("Bass"));
        (e, a, b)
    }

    fn eq_of(strip: &Value) -> Value {
        strip["inserts"].as_array().unwrap().iter().find(|i| i["plugin"] == "eq_7band").cloned().unwrap()
    }

    fn strip(v: &Value, id: u64) -> Value {
        v["strips"].as_array().unwrap().iter().find(|s| s["id"] == id).cloned().unwrap()
    }

    #[test]
    fn the_snapshot_matches_session_inspect() {
        let (mut e, a, _) = engine();
        e.execute("mix.volume", &json!({"track": a, "db": -6.5})).unwrap();
        e.execute("mix.pan", &json!({"track": a, "pan": -0.25})).unwrap();
        e.execute("mix.mute", &json!({"tracks": [a], "value": true})).unwrap();
        e.execute("mix.insert", &json!({"track": a, "plugin": "eq_7band"})).unwrap();
        let st = mixer_state(&e);
        let ins = crate::inspect::session(&e, true);
        for t in ins["tracks"].as_array().unwrap() {
            if t["kind"] == "video" {
                continue;
            }
            let s = strip(&st, t["id"].as_u64().unwrap());
            for k in ["name", "kind", "mute", "solo", "record_arm", "input", "output", "automation_mode"] {
                assert_eq!(s[k], t[k], "{k}");
            }
            assert!((s["volume_db"].as_f64().unwrap() - t["volume_db"].as_f64().unwrap()).abs() < 0.01);
            for (x, y) in s["pan"].as_array().unwrap().iter().zip(t["pan"].as_array().unwrap()) {
                assert!((x.as_f64().unwrap() - y.as_f64().unwrap()).abs() < 0.01);
            }
            assert_eq!(s["inserts"].as_array().unwrap().len(), t["inserts"].as_array().unwrap().len());
            for (x, y) in s["inserts"].as_array().unwrap().iter().zip(t["inserts"].as_array().unwrap()) {
                assert_eq!((&x["module"], &x["plugin"], &x["bypass"]), (&y["module"], &y["plugin"], &y["bypass"]));
            }
        }
        let s = strip(&st, a);
        assert_eq!(s["volume_db"], -6.5);
        assert_eq!(s["mute"], true);
        let eq = &eq_of(&s);
        assert_eq!(eq["hosted"], false);
        assert!(eq["params"].as_object().is_some_and(|p| !p.is_empty()), "a built-in carries its parameters: {eq}");
        assert!(st["strips"].as_array().unwrap().iter().any(|s| s["kind"] == "master"), "the master is a strip");
        assert_eq!(st["undo"], e.undo_label().map_or(Value::Null, |l| json!(l)), "the next undo, by label");
    }

    #[test]
    fn undo_labels_and_snapshots_are_in_the_feed() {
        let (mut e, a, _) = engine();
        let mut mirror = mixer_state(&e);
        e.execute("mix.volume", &json!({"track": a, "db": -2.0})).unwrap();
        let d = mixer_delta(&mirror, &mixer_state(&e)).unwrap();
        assert_eq!(d["undo"], json!(e.undo_label().unwrap()), "{d}");
        apply_delta(&mut mirror, &d);
        e.execute("mix.snapshot_store", &json!({"name": "Sermon"})).unwrap();
        let d = mixer_delta(&mirror, &mixer_state(&e)).unwrap();
        assert!(d["snapshots"].as_array().unwrap().iter().any(|x| x["name"] == "Sermon"), "{d}");
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(mixer_state(&e)["redo"].is_string());
    }

    #[test]
    fn deltas_carry_only_what_changed_from_any_path_undo_and_snapshot_recall_included() {
        let (mut e, a, b) = engine();
        e.execute("mix.insert", &json!({"track": a, "plugin": "eq_7band"})).unwrap();
        let mut mirror = mixer_state(&e);
        let step = |e: &Engine, mirror: &mut Value, what: &str| -> Value {
            let now = mixer_state(e);
            let d = mixer_delta(mirror, &now).unwrap_or_else(|| panic!("{what}: no change"));
            apply_delta(mirror, &d);
            assert_eq!(*mirror, now, "the mirror follows");
            d
        };
        assert!(mixer_delta(&mirror, &mixer_state(&e)).is_none(), "nothing changed, nothing sent");
        // Fader (as the Mix window drags it: merged).
        e.execute_merged("mix.volume", &json!({"track": a, "db": -4.25}), "fader").unwrap();
        e.end_merge();
        let d = step(&e, &mut mirror, "fader");
        assert_eq!(d["strips"], json!([{"id": a, "volume_db": -4.25}]), "{d}");
        // Mute.
        e.execute("mix.mute", &json!({"tracks": [b], "value": true})).unwrap();
        let d = step(&e, &mut mirror, "mute");
        assert_eq!(d["strips"], json!([{"id": b, "mute": true}]));
        // A send.
        e.execute("mix.send", &json!({"track": a, "slot": 5, "bus": "Verb", "level_db": -10.0})).unwrap();
        step(&e, &mut mirror, "send");
        e.execute("mix.send_level", &json!({"track": a, "slot": 5, "db": -12.0})).unwrap();
        let d = step(&e, &mut mirror, "send level");
        let sends = d["strips"][0]["sends"].as_array().unwrap().clone();
        assert!(sends.iter().any(|x| x["slot"] == 5 && x["level_db"] == -12.0), "{d}");
        // An insert parameter, in plain units.
        let module = eq_of(&strip(&mirror, a))["module"].as_u64().unwrap();
        let p = crate::cmd::plugin_info("eq_7band").unwrap().params.iter().find(|p| p.max > p.min && p.max - p.min > 1.0).unwrap();
        let v = p.min + (p.max - p.min) * 0.25;
        e.execute("mix.module_param", &json!({"track": a, "module": module, "param": p.id, "value": v})).unwrap();
        let d = step(&e, &mut mirror, "insert param");
        let got = eq_of(&d["strips"][0])["params"][p.id].as_f64().unwrap();
        assert!((got - f64::from(v)).abs() < 0.01, "{got} vs {v}");
        // Undo brings the parameter back, as a delta.
        e.execute("edit.undo", &json!({})).unwrap();
        let d = step(&e, &mut mirror, "undo");
        assert!(d["strips"][0]["inserts"].is_array(), "{d}");
        // A mix snapshot recalled: every fader it moves.
        e.execute("mix.snapshot_store", &json!({"name": "A"})).unwrap();
        e.execute("mix.volume", &json!({"track": b, "db": -20.0})).unwrap();
        step(&e, &mut mirror, "volume");
        e.execute("mix.snapshot_recall", &json!({"snapshot": "A"})).unwrap();
        let d = step(&e, &mut mirror, "recall");
        assert!(d["strips"].as_array().unwrap().iter().any(|s| s["id"] == b && s.get("volume_db").is_some()), "{d}");
        // A new track arrives whole, a deleted one as `removed`.
        e.execute("track.new", &json!({"name": "Keys"})).unwrap();
        let d = step(&e, &mut mirror, "new track");
        assert!(d["strips"][0]["name"].as_str().is_some_and(|n| n.starts_with("Keys")) && d.get("order").is_some(), "{d}");
        e.execute("track.delete", &json!({"tracks": [b]})).unwrap();
        let d = step(&e, &mut mirror, "delete");
        assert_eq!(d["removed"], json!([b]));
    }

    #[test]
    fn a_remote_fader_drag_is_one_undo_step_across_frames_until_released() {
        let (mut e, a, _) = engine();
        let db = |e: &Engine| e.session().tracks.iter().find(|t| t.id.0 == a).unwrap().mixer.volume_db;
        let before = db(&e);
        for (k, v) in [-1.0, -2.0, -3.0].into_iter().enumerate() {
            e.execute_gesture("mix.volume", &json!({"track": a, "db": v}), "fader").unwrap();
            // The booth's frames go by with no pointer down: the step stays open.
            if k < 2 {
                e.end_idle_merge();
            }
        }
        e.end_merge();
        assert_eq!(db(&e), -3.0);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(db(&e), before, "one undo step for the whole drag");
        // A remote that never lets go: its step ends once the lease runs out.
        e.execute_gesture("mix.volume", &json!({"track": a, "db": -5.0}), "fader").unwrap();
        std::thread::sleep(Engine::GESTURE_LEASE + std::time::Duration::from_millis(50));
        e.end_idle_merge();
        e.execute_gesture("mix.volume", &json!({"track": a, "db": -7.0}), "fader").unwrap();
        e.end_merge();
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(db(&e), -5.0, "two steps: the first ended with its lease");
    }

    #[test]
    fn a_remote_release_ends_only_its_own_gesture() {
        let (mut e, a, b) = engine();
        e.execute_gesture("mix.volume", &json!({"track": a, "db": -1.0}), "fader:a").unwrap();
        e.end_gesture("fader:b");
        assert!(e.gesture_open("fader:a"), "another control's release leaves this drag alone");
        e.end_gesture("fader:a");
        assert!(!e.gesture_open("fader:a"));
        // A local drag under way is not ended by a remote's release either.
        let start = e.session().tracks.iter().find(|t| t.id.0 == b).unwrap().mixer.volume_db;
        e.execute_merged("mix.volume", &json!({"track": b, "db": -2.0}), "fader").unwrap();
        e.end_gesture("fader:a");
        e.execute_merged("mix.volume", &json!({"track": b, "db": -3.0}), "fader").unwrap();
        e.end_merge();
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().tracks.iter().find(|t| t.id.0 == b).unwrap().mixer.volume_db, start, "one step");
    }

    #[test]
    fn meters_are_compact() {
        // 0 dB, -6 dB, silence, -12 dB; 3 dB of gain reduction.
        let v = meters_json(1.23456, &[(3, StripLevels { peak: [1.0, 0.5], rms: [0.0, 0.25], gr_db: -3.04 })]);
        assert_eq!(level_byte(1.0), 200);
        assert_eq!(level_byte(0.5), 188);
        assert_eq!(level_byte(0.25), 176);
        assert_eq!(v, json!({"pos": 1.235, "ids": [3], "lv": base64(&[200, 188, 0, 176, 12])}));
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
    }
}
