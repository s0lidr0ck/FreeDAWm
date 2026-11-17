//! Mixer snapshots: store the strips' settings under a name, recall them whole or by section
//! (faders, pan, mutes, sends, inserts, routing, delay) on every track or a chosen few. Each
//! store and recall is one undo step. Window › Mix Snapshots is their window.

use super::*;
use crate::cmd;
use serde_json::{Value, json};
use soundcraft_model::{MAX_SNAPSHOTS, MixSnapshot, SNAPSHOT_SECTIONS};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.snapshot_store",
            "Store Mix Snapshot",
            [],
            None,
            "{name?, replace?: index|name} — every strip's settings now, as a new snapshot (or in place of one) → {index, name}",
            always,
            store
        ),
        cmd!(
            "mix.snapshot_recall",
            "Recall Mix Snapshot",
            [],
            None,
            "{snapshot: index|name, sections?: [faders|pan|mutes|sends|inserts|routing|delay] (default: all), tracks?} → {recalled: track count}",
            always,
            recall
        ),
        cmd!("mix.snapshot_delete", "Delete Mix Snapshot", [], None, "{snapshot: index|name}", always, |e, p| {
            let i = snapshot_index(e, "mix.snapshot_delete", p.get("snapshot"))?;
            let gone = e.session_mut().mix_snapshots.remove(i);
            Ok(json!({"deleted": gone.name}))
        }),
        cmd!(query "mix.snapshots", "Mix Snapshots", [], None, "{} → [{index, name, tracks}]", always, |e, _| {
            Ok(Value::Array(
                e.session().mix_snapshots.iter().enumerate().map(|(i, s)| json!({"index": i, "name": s.name, "tracks": s.strips.len()})).collect(),
            ))
        }),
    ]
}

fn snapshot_index(e: &Engine, c: &str, v: Option<&Value>) -> Result<usize> {
    let snaps = &e.session().mix_snapshots;
    let i = match v {
        Some(Value::Number(n)) => n.as_u64().and_then(|n| usize::try_from(n).ok()),
        Some(Value::String(name)) => snaps.iter().position(|s| s.name.eq_ignore_ascii_case(name)),
        _ => return Err(bad(c, "`snapshot` (index or name) required")),
    };
    i.filter(|i| *i < snaps.len()).ok_or_else(|| bad(c, "no such snapshot"))
}

fn store(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.snapshot_store";
    let replace = match p.get("replace") {
        None | Some(Value::Null) => None,
        v => Some(snapshot_index(e, C, v)?),
    };
    let s = e.session();
    let name = match str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.chars().take(64).collect(),
        None => match replace.and_then(|i| s.mix_snapshots.get(i)) {
            Some(old) => old.name.clone(),
            None => {
                let mut n = s.mix_snapshots.len() + 1;
                while s.mix_snapshots.iter().any(|x| x.name == format!("Snapshot {n}")) {
                    n += 1;
                }
                format!("Snapshot {n}")
            }
        },
    };
    let snap = MixSnapshot::take(&name, s.tracks.iter());
    let snaps = &mut e.session_mut().mix_snapshots;
    let index = match replace {
        Some(i) => {
            if let Some(x) = snaps.get_mut(i) {
                *x = snap;
            }
            i
        }
        None => {
            if snaps.len() >= MAX_SNAPSHOTS {
                return Err(bad(C, format!("a session keeps at most {MAX_SNAPSHOTS} snapshots")));
            }
            snaps.push(snap);
            snaps.len() - 1
        }
    };
    Ok(json!({"index": index, "name": name}))
}

fn recall(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.snapshot_recall";
    let i = snapshot_index(e, C, p.get("snapshot"))?;
    let sections: Vec<&str> = match p.get("sections") {
        None | Some(Value::Null) => SNAPSHOT_SECTIONS.iter().map(|(id, _)| *id).collect(),
        Some(Value::Array(a)) => {
            let mut out = Vec::new();
            for v in a {
                let id = v.as_str().and_then(|x| SNAPSHOT_SECTIONS.iter().find(|(id, _)| id.eq_ignore_ascii_case(x))).map(|(id, _)| *id);
                out.push(id.ok_or_else(|| bad(C, format!("unknown section {v} (faders, pan, mutes, sends, inserts, routing or delay)")))?);
            }
            out
        }
        Some(_) => return Err(bad(C, "`sections` is a list")),
    };
    let only: Option<Vec<TrackId>> = if p.get("tracks").is_some() || p.get("track").is_some() { Some(tracks_required(e, C, p)?) } else { None };
    let Some(snap) = e.session().mix_snapshots.get(i).cloned() else { return Err(bad(C, "no such snapshot")) };
    let mut n = 0usize;
    let mut changed = Vec::new();
    for t in &mut e.session_mut().tracks {
        if only.as_ref().is_some_and(|o| !o.contains(&t.id)) {
            continue;
        }
        if snap.recall_onto(t, &sections) {
            n += 1;
            changed.push(t.id);
        }
    }
    // Recalled inserts are new modules to the engine: their lanes and instances follow.
    e.session_mut().sanitize();
    for t in changed {
        e.automation_changed(t, soundcraft_model::AutoParam::Volume);
    }
    Ok(json!({"recalled": n, "name": snap.name}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, TrackKind};

    #[test]
    fn a_snapshot_stores_and_recalls_by_section_and_track() {
        let mut e = Engine::default();
        let a = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("A"));
        let b = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("B"));
        e.execute("mix.volume", &json!({"tracks": [a.0, b.0], "db": -6.0})).unwrap();
        e.execute("mix.pan", &json!({"track": a.0, "pan": -0.5})).unwrap();
        let r = e.execute("mix.snapshot_store", &json!({"name": "Verse"})).unwrap();
        assert_eq!((r["index"].as_u64(), r["name"].as_str()), (Some(0), Some("Verse")));
        e.execute("mix.volume", &json!({"tracks": [a.0, b.0], "db": 0.0})).unwrap();
        e.execute("mix.pan", &json!({"track": a.0, "pan": 0.5})).unwrap();
        let mx = |e: &Engine, t: TrackId| e.session().track(t).unwrap().mixer.clone();
        // Faders only, on A only: the pan stays, B stays.
        e.execute("mix.snapshot_recall", &json!({"snapshot": "verse", "sections": ["faders"], "tracks": [a.0]})).unwrap();
        assert_eq!((mx(&e, a).volume_db, mx(&e, a).pan[0], mx(&e, b).volume_db), (-6.0, 0.5, 0.0));
        // Everything, everywhere; one undo step undoes it.
        assert_eq!(e.execute("mix.snapshot_recall", &json!({"snapshot": 0})).unwrap()["recalled"], 2);
        assert_eq!((mx(&e, a).pan[0], mx(&e, b).volume_db), (-0.5, -6.0));
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(mx(&e, b).volume_db, 0.0);
        assert!(e.execute("mix.snapshot_recall", &json!({"snapshot": "Chorus"})).is_err());
        assert!(e.execute("mix.snapshot_recall", &json!({"snapshot": 0, "sections": ["eq"]})).is_err());
        // Saved with the session; deleted by name.
        let back: soundcraft_model::Session = serde_json::from_str(&serde_json::to_string(e.session()).unwrap()).unwrap();
        assert_eq!(back.mix_snapshots.len(), 1);
        e.execute("mix.snapshot_delete", &json!({"snapshot": "Verse"})).unwrap();
        assert!(e.session().mix_snapshots.is_empty());
    }
}
