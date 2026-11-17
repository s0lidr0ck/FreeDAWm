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
        cmd!(
            "track.midi_fx_record",
            "Record Effects' Output",
            [],
            None,
            "{tracks?, value?: bool (default: toggle the first)} — takes on these tracks keep what their MIDI effects make of the keys (the clip then plays past the effects) instead of the keys as played → {value}",
            has_selection,
            record
        ),
        cmd!(
            "track.mpe",
            "MPE Zone",
            [],
            None,
            "{tracks?, zone?: off|lower|upper, members?: 1..15, bend?: semitones 0..96 (members, default 48), master_bend?: 0..96 (default 2)} — sets the tracks' instruments up for MPE: the MPE Configuration Message and pitch-bend ranges are sent to them → {mpe}",
            has_selection,
            mpe
        ),
        cmd!(
            "track.articulations",
            "Articulations",
            [],
            None,
            "{tracks?, add?: {name, keyswitch: note}, index?: n (with name?/keyswitch? to change it, or remove: true), cc?: 0..119 (the lane that chooses, default 3), clear?: bool} — the tracks' sound variations: articulations switched by keyswitch, chosen in clips by a controller lane whose value is the articulation's index → {articulations}",
            has_selection,
            articulations
        ),
    ]
}

fn articulations(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "track.articulations";
    let tracks = midi_tracks(e, C, p)?;
    let first = tracks.first().and_then(|t| e.session().track(*t)).and_then(|t| t.articulations.clone());
    let note = |v: &Value, k: &str| -> Result<Option<u8>> {
        match v.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(n) => {
                n.as_u64().filter(|n| *n <= 127).map(|n| Some(n as u8)).ok_or_else(|| bad(C, format!("`{k}` is a MIDI note number, 0 to 127")))
            }
        }
    };
    let name = |v: &Value| v.get("name").and_then(Value::as_str).map(|n| n.trim().chars().take(60).collect::<String>()).filter(|n| !n.is_empty());
    let mut a = first.unwrap_or_default();
    if p.get("clear").and_then(Value::as_bool) == Some(true) {
        for t in &tracks {
            if let Some(tr) = e.session_mut().track_mut(*t) {
                tr.articulations = None;
            }
        }
        return Ok(json!({"articulations": null}));
    }
    if let Some(cc) = note(p, "cc")? {
        if cc > 119 {
            return Err(bad(C, "`cc` is 0 to 119 (120 and up are channel mode messages)"));
        }
        a.cc = cc;
    }
    if let Some(add) = p.get("add") {
        let n = name(add).ok_or_else(|| bad(C, "`add` needs a `name`"))?;
        let k = note(add, "keyswitch")?.ok_or_else(|| bad(C, "`add` needs a `keyswitch` note"))?;
        if a.items.len() >= soundcraft_model::MAX_ARTICULATIONS {
            return Err(bad(C, "a track holds at most 128 articulations"));
        }
        a.items.push(soundcraft_model::Articulation { name: n, keyswitch: k });
    }
    if let Some(i) = p.get("index").and_then(Value::as_u64) {
        let i = usize::try_from(i).unwrap_or(usize::MAX);
        if i >= a.items.len() {
            return Err(bad(C, format!("no articulation {i}: the track has {}", a.items.len())));
        }
        if p.get("remove").and_then(Value::as_bool) == Some(true) {
            a.items.remove(i);
        } else if let Some(it) = a.items.get_mut(i) {
            if let Some(n) = name(p) {
                it.name = n;
            }
            if let Some(k) = note(p, "keyswitch")? {
                it.keyswitch = k;
            }
        }
    }
    for t in &tracks {
        if let Some(tr) = e.session_mut().track_mut(*t) {
            tr.articulations = Some(a.clone());
        }
    }
    Ok(json!({"articulations": a}))
}

fn mpe(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "track.mpe";
    let tracks = midi_tracks(e, C, p)?;
    let now = tracks.first().and_then(|t| e.session().track(*t)).and_then(|t| t.mpe);
    let num = |k: &str, max: u64| -> Result<Option<u8>> {
        match p.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v.as_u64().filter(|n| *n <= max).map(|n| Some(n as u8)).ok_or_else(|| bad(C, format!("`{k}` is a whole number up to {max}"))),
        }
    };
    let (members, bend, master_bend) = (num("members", 15)?, num("bend", 96)?, num("master_bend", 96)?);
    if members == Some(0) {
        return Err(bad(C, "`members` is 1 to 15 (zone `off` turns MPE off)"));
    }
    let zone = match p.get("zone").and_then(Value::as_str) {
        Some("off") => None,
        Some("lower") => Some(false),
        Some("upper") => Some(true),
        Some(z) => return Err(bad(C, format!("unknown zone `{z}`: off, lower or upper"))),
        None => Some(now.is_some_and(|z| z.upper)),
    };
    let value = zone.map(|upper| {
        let base = now.unwrap_or_default();
        soundcraft_model::MpeZone {
            upper,
            members: members.unwrap_or(base.members),
            bend: bend.unwrap_or(base.bend),
            master_bend: master_bend.unwrap_or(base.master_bend),
        }
    });
    for t in &tracks {
        if let Some(tr) = e.session_mut().track_mut(*t) {
            tr.mpe = value;
        }
    }
    Ok(json!({"mpe": value}))
}

fn record(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "track.midi_fx_record";
    let tracks = midi_tracks(e, C, p)?;
    let now = tracks.first().and_then(|t| e.session().track(*t)).is_some_and(|t| t.midi_fx_record);
    let value = match p.get("value") {
        None | Some(Value::Null) => !now,
        Some(v) => v.as_bool().ok_or_else(|| bad(C, "`value` is true or false"))?,
    };
    for t in &tracks {
        if let Some(tr) = e.session_mut().track_mut(*t) {
            tr.midi_fx_record = value;
        }
    }
    Ok(json!({"value": value}))
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
    fn articulations_are_added_changed_removed_and_undone() {
        let mut e = Engine::default();
        let id = e.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        let arts = |e: &Engine| e.session().track(soundcraft_model::TrackId(id)).unwrap().articulations.clone();
        e.execute("track.articulations", &json!({"tracks": [id], "add": {"name": "Legato", "keyswitch": 24}})).unwrap();
        let r = e.execute("track.articulations", &json!({"tracks": [id], "add": {"name": "Pizzicato", "keyswitch": 26}})).unwrap();
        assert_eq!(r["articulations"]["cc"], 3, "the lane that chooses, by default");
        e.execute("track.articulations", &json!({"tracks": [id], "index": 1, "name": "Staccato", "keyswitch": 25})).unwrap();
        let a = arts(&e).unwrap();
        assert_eq!(a.items.iter().map(|i| (i.name.as_str(), i.keyswitch)).collect::<Vec<_>>(), vec![("Legato", 24), ("Staccato", 25)]);
        assert!(e.execute("track.articulations", &json!({"tracks": [id], "cc": 121})).is_err());
        assert!(e.execute("track.articulations", &json!({"tracks": [id], "index": 5, "remove": true})).is_err());
        assert!(e.execute("track.articulations", &json!({"tracks": [id], "add": {"name": "X"}})).is_err());
        e.execute("track.articulations", &json!({"tracks": [id], "index": 0, "remove": true})).unwrap();
        assert_eq!(arts(&e).unwrap().items.len(), 1);
        assert!(e.undo());
        assert_eq!(arts(&e).unwrap().items.len(), 2);
        e.execute("track.articulations", &json!({"tracks": [id], "clear": true})).unwrap();
        assert_eq!(arts(&e), None);
    }

    #[test]
    fn an_mpe_zone_is_set_changed_turned_off_and_undone() {
        let mut e = Engine::default();
        let id = e.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        let zone = |e: &Engine| e.session().track(soundcraft_model::TrackId(id)).unwrap().mpe;
        assert_eq!(zone(&e), None);
        e.execute("track.mpe", &json!({"tracks": [id], "zone": "lower"})).unwrap();
        assert_eq!(zone(&e), Some(soundcraft_model::MpeZone::default()), "15 members, 48 semitones, master 2");
        let r = e.execute("track.mpe", &json!({"tracks": [id], "zone": "upper", "members": 7, "bend": 24})).unwrap();
        assert_eq!(r["mpe"]["members"], 7);
        assert_eq!(zone(&e).map(|z| (z.upper, z.members, z.bend, z.master_bend)), Some((true, 7, 24, 2)));
        assert!(e.execute("track.mpe", &json!({"tracks": [id], "members": 0})).is_err());
        assert!(e.execute("track.mpe", &json!({"tracks": [id], "bend": 97})).is_err());
        e.execute("track.mpe", &json!({"tracks": [id], "zone": "off"})).unwrap();
        assert_eq!(zone(&e), None);
        assert!(e.undo());
        assert_eq!(zone(&e).map(|z| z.members), Some(7));
        let audio = e.execute("track.new", &json!({"kind": "audio"})).unwrap()["tracks"][0].as_u64().unwrap();
        assert!(e.execute("track.mpe", &json!({"tracks": [audio], "zone": "lower"})).is_err(), "no instrument");
    }

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
