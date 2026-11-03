//! Session format migrations, applied to the raw JSON before deserialising.

use serde_json::{Value, json};

/// Upgrades a format-1 session document to format 2 in place.
///
/// Per track: the fixed-slot `mixer.inserts` array becomes a dense chain (nulls removed, slot
/// order kept) with module ids 1..n, and plugin automation lanes move from slot numbers to those
/// ids. Lanes pointing at an empty slot are removed; the trim lane becomes `Trim`.
pub fn migrate_v1(v: &mut Value) {
    let Some(tracks) = v.get_mut("tracks").and_then(Value::as_array_mut) else { return };
    for track in tracks {
        migrate_track(track);
    }
}

fn migrate_track(track: &mut Value) {
    // slot index -> module id, for the slots that held a plugin.
    let mut slot_ids: Vec<Option<u64>> = Vec::new();
    if let Some(mixer) = track.get_mut("mixer").and_then(Value::as_object_mut) {
        let old = match mixer.get("inserts") {
            Some(Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        let mut dense = Vec::new();
        let mut next = 1u64;
        for slot in old {
            if let Value::Object(mut ins) = slot {
                ins.insert("id".into(), json!(next));
                slot_ids.push(Some(next));
                dense.push(Value::Object(ins));
                next += 1;
            } else {
                slot_ids.push(None);
            }
        }
        mixer.insert("inserts".into(), Value::Array(dense));
        mixer.insert("next_module".into(), json!(next));
    }
    migrate_view(track, &slot_ids);
    let Some(lanes) = track.get_mut("automation").and_then(Value::as_array_mut) else { return };
    lanes.retain_mut(|lane| {
        let Some(param) = lane.get_mut("param") else { return true };
        let Some(plugin) = param.get("Plugin") else { return true };
        let name = plugin.get("param").and_then(Value::as_str).unwrap_or("").to_string();
        let slot = plugin.get("slot").and_then(Value::as_u64);
        if slot == Some(255) && name == "trim" {
            *param = json!("Trim");
            return true;
        }
        let id = slot.and_then(|s| usize::try_from(s).ok()).and_then(|s| slot_ids.get(s).copied().flatten());
        match id {
            Some(id) => {
                *param = json!({"Plugin": {"module": id, "param": name}});
                true
            }
            None => false,
        }
    });
}

/// Rewrites a persisted `plugin:<letter|index>:<param>` view to the module-id form (`volume` when
/// that slot was empty, `trim` for the trim lane).
fn migrate_view(track: &mut Value, slot_ids: &[Option<u64>]) {
    let Some(view) = track.get("view").and_then(Value::as_str) else { return };
    let lower = view.trim().to_ascii_lowercase();
    let Some((pos, param)) = lower.strip_prefix("plugin:").and_then(|r| r.split_once(':')) else { return };
    let slot = crate::letter_slot(pos).map(usize::from);
    let new = if slot == Some(255) && param == "trim" {
        "trim".to_string()
    } else {
        match slot.and_then(|s| slot_ids.get(s).copied().flatten()) {
            Some(id) => format!("plugin:m{id}:{param}"),
            None => "volume".to_string(),
        }
    };
    track["view"] = json!(new);
}

#[cfg(test)]
mod tests {
    use crate::{AutoParam, ChannelFormat, Session, TrackKind};
    use serde_json::{Value, json};

    fn insert(plugin: &str) -> Value {
        json!({"plugin": plugin, "params": {}, "bypass": false, "active": true, "preset": ""})
    }

    fn v1_fixture() -> String {
        let mut s = Session::default();
        s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let mut v = serde_json::to_value(&s).unwrap();
        v["format_version"] = json!(1);
        let t = &mut v["tracks"][0];
        let mut inserts = vec![Value::Null; 10];
        inserts[0] = insert("gain");
        inserts[2] = insert("compressor");
        inserts[9] = insert("eq_7band");
        t["mixer"]["inserts"] = Value::Array(inserts);
        if let Some(m) = t["mixer"].as_object_mut() {
            m.remove("next_module");
        }
        t["view"] = json!("plugin:c:threshold");
        let lane = |p: Value| json!({"param": p, "points": [], "enabled": true});
        t["automation"] = json!([
            lane(json!({"Plugin": {"slot": 2, "param": "threshold"}})),
            lane(json!({"Plugin": {"slot": 255, "param": "trim"}})),
            lane(json!({"Plugin": {"slot": 4, "param": "gain"}})),
        ]);
        v.to_string()
    }

    #[test]
    fn v1_session_with_gaps_migrates() {
        let s = Session::from_json(&v1_fixture()).unwrap();
        let m = &s.tracks[0].mixer;
        assert_eq!(m.inserts.iter().map(|i| i.plugin.as_str()).collect::<Vec<_>>(), ["gain", "compressor", "eq_7band"]);
        let ids: Vec<u32> = m.inserts.iter().map(|i| i.id.0).collect();
        assert_eq!(ids, [1, 2, 3]);
        assert_eq!(m.next_module, 4);
        let comp = m.inserts[1].id;
        let params: Vec<&AutoParam> = s.tracks[0].automation.iter().map(|l| &l.param).collect();
        assert!(params.contains(&&AutoParam::Plugin { module: comp, param: "threshold".into() }));
        assert!(params.contains(&&AutoParam::Trim));
        assert_eq!(params.len(), 2);
        assert_eq!(s.format_version, Session::FORMAT_VERSION);
        assert_eq!(s.tracks[0].view, format!("plugin:m{}:threshold", comp.0));
        assert_eq!(AutoParam::parse(&s.tracks[0].view), Some(AutoParam::Plugin { module: comp, param: "threshold".into() }));
    }

    #[test]
    fn v1_view_on_an_empty_slot_falls_back_to_volume() {
        let mut v: Value = serde_json::from_str(&v1_fixture()).unwrap();
        v["tracks"][0]["view"] = json!("plugin:b:gain");
        let s = Session::from_json(&v.to_string()).unwrap();
        assert_eq!(s.tracks[0].view, "volume");
    }
}
