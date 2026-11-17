//! MIDI effects ahead of a track's instrument (`soundcraft_model::MidiFx`): an arpeggiator, a
//! chord, a scale and note repeat, run by the mix on everything the instrument is given (clips,
//! live input, routed MIDI tracks). Instrument and MIDI tracks only; at most
//! [`soundcraft_model::MAX_MIDI_FX`] per track. Each command is one undo step (a drag in the
//! menu's controls is merged into one by the UI).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{ARP_PATTERNS, CHORD_SHAPES, MAX_MIDI_FX, MIDI_FX_RATES, MidiFx, MidiFxKind, SCALES};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "track.midi_fx_add",
            "Add MIDI Effect",
            [],
            None,
            "{tracks?, kind: arpeggiator | chord | scale | note_repeat, at?: index} — a MIDI effect ahead of the instrument of each instrument or MIDI track, last in its list or at `at` (at most 4) → {midi_fx}",
            has_selection,
            add
        ),
        cmd!(
            "track.midi_fx_set",
            "Set MIDI Effect",
            [],
            None,
            "{tracks?, index, bypass?, rate?: 0..6 (1/4 1/8 1/8T 1/16 1/16T 1/32 1/32T), pattern?: 0..4 (up down up/down as-played random), octaves?: 1..4, gate?: 5..100 (%), latch?, chord?: 0..11, root?: 0..11, scale?: 0..11, drop?, kind?, to?: index (move it there)} — a MIDI effect's settings → {midi_fx}",
            has_selection,
            set
        ),
        cmd!(
            "track.midi_fx_remove",
            "Remove MIDI Effect",
            [],
            None,
            "{tracks?, index?: (default: all)} — takes a MIDI effect (or all) off the tracks → {midi_fx}",
            has_selection,
            remove
        ),
    ]
}

/// The tracks, each an instrument or MIDI track.
fn midi_tracks(e: &Engine, c: &str, p: &Value) -> Result<Vec<TrackId>> {
    let tracks = tracks_required(e, c, p)?;
    if let Some(t) = tracks.iter().filter_map(|t| e.session().track(*t)).find(|t| !t.kind.is_midi()) {
        return Err(bad(c, format!("`{}` has no instrument: MIDI effects go on instrument and MIDI tracks", t.name)));
    }
    Ok(tracks)
}

fn list_of(e: &Engine, t: Option<&TrackId>) -> Value {
    json!(t.and_then(|t| e.session().track(*t)).map(|t| t.midi_fx.clone()).unwrap_or_default())
}

fn add(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "track.midi_fx_add";
    let tracks = midi_tracks(e, C, p)?;
    let kind = str_param(p, "kind").and_then(MidiFxKind::from_key).ok_or_else(|| bad(C, "`kind` is arpeggiator, chord, scale or note_repeat"))?;
    if let Some(t) = tracks.iter().filter_map(|t| e.session().track(*t)).find(|t| t.midi_fx.len() >= MAX_MIDI_FX) {
        return Err(bad(C, format!("`{}` already has {MAX_MIDI_FX} MIDI effects", t.name)));
    }
    let at = p.get("at").and_then(Value::as_u64);
    for t in &tracks {
        if let Some(tr) = e.session_mut().track_mut(*t) {
            let i = at.map_or(tr.midi_fx.len(), |a| usize::try_from(a).unwrap_or(usize::MAX).min(tr.midi_fx.len()));
            tr.midi_fx.insert(i, MidiFx::new(kind));
        }
    }
    Ok(json!({"midi_fx": list_of(e, tracks.first())}))
}

fn set(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "track.midi_fx_set";
    let tracks = midi_tracks(e, C, p)?;
    let index =
        p.get("index").and_then(Value::as_u64).and_then(|i| usize::try_from(i).ok()).ok_or_else(|| bad(C, "`index`: which MIDI effect (0 first)"))?;
    if let Some(t) = tracks.iter().filter_map(|t| e.session().track(*t)).find(|t| index >= t.midi_fx.len()) {
        return Err(bad(C, format!("`{}` has {} MIDI effect(s); no index {index}", t.name, t.midi_fx.len())));
    }
    let int = |key: &str, lo: i64, hi: i64| -> Result<Option<u8>> {
        match p.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_f64()
                .map(|x| x.round() as i64)
                .filter(|x| (lo..=hi).contains(x))
                .and_then(|x| u8::try_from(x).ok())
                .map(Some)
                .ok_or_else(|| bad(C, format!("`{key}` is {lo} to {hi}"))),
        }
    };
    let flag = |key: &str| -> Result<Option<bool>> {
        match p.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v.as_bool().map(Some).ok_or_else(|| bad(C, format!("`{key}` is true or false"))),
        }
    };
    let rate = int("rate", 0, MIDI_FX_RATES.len() as i64 - 1)?;
    let pattern = int("pattern", 0, ARP_PATTERNS.len() as i64 - 1)?;
    let octaves = int("octaves", 1, 4)?;
    let gate = int("gate", 5, 100)?;
    let chord = int("chord", 0, CHORD_SHAPES.len() as i64 - 1)?;
    let root = int("root", 0, 11)?;
    let scale = int("scale", 0, SCALES.len() as i64 - 1)?;
    let (bypass, latch, drop) = (flag("bypass")?, flag("latch")?, flag("drop")?);
    let kind = match str_param(p, "kind") {
        None => None,
        Some(k) => Some(MidiFxKind::from_key(k).ok_or_else(|| bad(C, "`kind` is arpeggiator, chord, scale or note_repeat"))?),
    };
    let to = p.get("to").and_then(Value::as_u64).map(|x| usize::try_from(x).unwrap_or(usize::MAX));
    for t in &tracks {
        let Some(tr) = e.session_mut().track_mut(*t) else { continue };
        let Some(f) = tr.midi_fx.get_mut(index) else { continue };
        let set = |dst: &mut u8, v: Option<u8>| {
            if let Some(v) = v {
                *dst = v;
            }
        };
        set(&mut f.rate, rate);
        set(&mut f.pattern, pattern);
        set(&mut f.octaves, octaves);
        set(&mut f.gate, gate);
        set(&mut f.chord, chord);
        set(&mut f.root, root);
        set(&mut f.scale, scale);
        f.bypass = bypass.unwrap_or(f.bypass);
        f.latch = latch.unwrap_or(f.latch);
        f.drop = drop.unwrap_or(f.drop);
        f.kind = kind.unwrap_or(f.kind);
        if let Some(to) = to {
            let fx = tr.midi_fx.remove(index);
            let to = to.min(tr.midi_fx.len());
            tr.midi_fx.insert(to, fx);
        }
    }
    Ok(json!({"midi_fx": list_of(e, tracks.first())}))
}

fn remove(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "track.midi_fx_remove";
    let tracks = midi_tracks(e, C, p)?;
    let index = p.get("index").and_then(Value::as_u64).map(|i| usize::try_from(i).unwrap_or(usize::MAX));
    for t in &tracks {
        let Some(tr) = e.session_mut().track_mut(*t) else { continue };
        match index {
            Some(i) if i < tr.midi_fx.len() => {
                tr.midi_fx.remove(i);
            }
            Some(_) => {}
            None => tr.midi_fx.clear(),
        }
    }
    Ok(json!({"midi_fx": list_of(e, tracks.first())}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::MidiFxKind;

    #[test]
    fn add_set_move_remove_and_undo() {
        let mut e = Engine::default();
        let id = e.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        let fx = |e: &Engine| e.session().track(soundcraft_model::TrackId(id)).unwrap().midi_fx.clone();
        e.execute("track.midi_fx_add", &json!({"tracks": [id], "kind": "chord"})).unwrap();
        e.execute("track.midi_fx_add", &json!({"tracks": [id], "kind": "arpeggiator", "at": 0})).unwrap();
        assert_eq!(fx(&e).iter().map(|f| f.kind).collect::<Vec<_>>(), vec![MidiFxKind::Arpeggiator, MidiFxKind::Chord]);
        let r = e.execute("track.midi_fx_set", &json!({"tracks": [id], "index": 0, "rate": 1, "latch": true, "octaves": 3, "to": 1})).unwrap();
        assert_eq!(r["midi_fx"][1]["kind"], "arpeggiator");
        let arp = fx(&e)[1];
        assert_eq!((arp.rate, arp.latch, arp.octaves), (1, true, 3));
        assert!(e.execute("track.midi_fx_set", &json!({"tracks": [id], "index": 0, "gate": 200})).is_err());
        assert!(e.execute("track.midi_fx_set", &json!({"tracks": [id], "index": 9, "gate": 50})).is_err());
        assert!(e.execute("track.midi_fx_add", &json!({"tracks": [id], "kind": "flanger"})).is_err());
        assert!(e.undo());
        assert_eq!(fx(&e)[0].kind, MidiFxKind::Arpeggiator, "the setting and the move undone together");
        assert!(e.redo());
        e.execute("track.midi_fx_remove", &json!({"tracks": [id], "index": 0})).unwrap();
        assert_eq!(fx(&e).len(), 1);
        e.execute("track.midi_fx_remove", &json!({"tracks": [id]})).unwrap();
        assert!(fx(&e).is_empty());
        for _ in 0..4 {
            e.execute("track.midi_fx_add", &json!({"tracks": [id], "kind": "scale"})).unwrap();
        }
        assert!(e.execute("track.midi_fx_add", &json!({"tracks": [id], "kind": "scale"})).is_err(), "four at most");
        // Kept with the session.
        let text = serde_json::to_string(e.session()).unwrap();
        let back: soundcraft_model::Session = serde_json::from_str(&text).unwrap();
        assert_eq!(back.track(soundcraft_model::TrackId(id)).unwrap().midi_fx, fx(&e));
        let audio = e.execute("track.new", &json!({"kind": "audio"})).unwrap()["tracks"][0].as_u64().unwrap();
        assert!(e.execute("track.midi_fx_add", &json!({"tracks": [audio], "kind": "chord"})).is_err());
    }
}
