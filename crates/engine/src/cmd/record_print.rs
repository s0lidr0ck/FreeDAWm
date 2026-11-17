//! `mix.record_inserts`: Record with Inserts (print). An armed audio track's take is what its
//! trim, polarity and inserts make of its input (after them, before the fader) instead of the
//! input as it came in. While it records, the input goes through the inserts whether the track is
//! monitored or not; with monitoring Off it is not heard.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::TrackKind;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "mix.record_inserts",
        "Record with Inserts",
        [],
        None,
        "{tracks?, value?: bool} → {value}. Audio tracks only; omitted, the value toggles from the first track's.",
        has_selection,
        record_inserts
    )]
}

fn record_inserts(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.record_inserts";
    let tracks = tracks_required(e, C, p)?;
    let s = e.session();
    let audio: Vec<TrackId> = tracks.iter().copied().filter(|t| s.track(*t).is_some_and(|t| t.kind == TrackKind::Audio)).collect();
    if audio.is_empty() {
        return Err(bad(C, "only an audio track records with its inserts"));
    }
    let v = match p.get("value") {
        None | Some(Value::Null) => !audio.first().and_then(|t| s.track(*t)).is_some_and(|t| t.mixer.record_inserts),
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(bad(C, "`value` is a bool")),
    };
    let s = e.session_mut();
    for t in &audio {
        if let Some(tr) = s.track_mut(*t) {
            tr.mixer.record_inserts = v;
        }
    }
    Ok(json!({"value": v}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_with_inserts_is_set_on_audio_tracks_only() {
        let mut e = Engine::default();
        let vox = e.execute("track.new", &json!({"name": "Vox", "format": "mono"})).unwrap()["tracks"][0].as_u64().unwrap();
        let keys = e.execute("track.new", &json!({"name": "Keys", "kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        assert_eq!(e.execute("mix.record_inserts", &json!({"tracks": [vox, keys]})).unwrap()["value"], true);
        assert!(e.session().track(TrackId(vox)).unwrap().mixer.record_inserts);
        assert!(!e.session().track(TrackId(keys)).unwrap().mixer.record_inserts);
        assert!(e.execute("mix.record_inserts", &json!({"tracks": [keys], "value": true})).is_err());
        // Toggles, and undoes.
        assert_eq!(e.execute("mix.record_inserts", &json!({"tracks": [vox]})).unwrap()["value"], false);
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().track(TrackId(vox)).unwrap().mixer.record_inserts);
        // Saved with the session.
        let json = e.session().to_json().unwrap();
        let back = soundcraft_model::Session::from_json(&json).unwrap();
        assert!(back.track(TrackId(vox)).unwrap().mixer.record_inserts);
    }
}
