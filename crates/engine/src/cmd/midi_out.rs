//! A track's MIDI output to hardware (`mix.midi_output`): an instrument or MIDI track plays a
//! hardware synth (or any device on a MIDI output port) through the External MIDI instrument
//! (`soundcraft_mix::external_midi`). The port is kept by name in the instrument's state, the
//! channel (0: as played, 1-16) as its parameter, so the session opens with it on any computer
//! that has a port of that name.
//!
//! A MIDI or instrument track can also play the instrument on another track
//! (`mix.midi_to_track`): its instrument becomes MIDI to Track (`soundcraft_mix::track_midi`), the
//! target's id kept in its state, the channel as its parameter. Several tracks on their own
//! channels so play one multi-timbral instrument; a route that would feed a track into itself is
//! refused. With `mix.midi_layers` one track plays up to four instruments at once, each over a key
//! range and a velocity range (layers overlap, splits sit side by side).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_mix::{external_midi, track_midi};
use soundcraft_model::Insert;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.midi_to_track",
            "Play Instrument On",
            [],
            None,
            "{track?, target: track | null, channel?: 0..16} — an instrument or MIDI track plays the instrument of another instrument or MIDI track (its own instrument becomes MIDI to Track), each event on `channel` (0: as played); target null takes it off (the track has no instrument). Refused when the target already plays into this track → {track, target, channel}",
            has_selection,
            set_to_track
        ),
        cmd!(
            "mix.midi_layers",
            "Layers and Splits",
            [],
            None,
            "{track?, layers: [{target: track, keys?: [lo, hi] (0-127, default all), velocities?: [lo, hi] (1-127, default all)}] (up to 4; [] takes them off)} — an instrument or MIDI track plays the instruments of up to four other instrument or MIDI tracks, a note reaching each one whose key and velocity ranges hold it (overlapping ranges layer, side by side ones split); its own instrument becomes MIDI to Track, the channel kept. Refused when a target already plays into this track → {track, layers}",
            has_selection,
            set_layers
        ),
        cmd!(
            "track.midi_record_on_target",
            "Record on the Target",
            [],
            None,
            "{tracks?, value?: bool (default: toggle the first)} — a take on these tracks, when they play other tracks' instruments (Play Instrument On, Layers & Splits), goes onto those tracks as each heard it (after this track's MIDI effects, each layer's key and velocity range, the route's channel) instead of staying on this track → {value}",
            has_selection,
            set_record_on_target
        ),
        cmd!(
            "mix.midi_output",
            "MIDI Output",
            [],
            None,
            "{track?, port: name | null, channel?: 0..16} — an instrument or MIDI track sends what it plays to a hardware MIDI output port (its instrument becomes External MIDI); channel 0 keeps each event's own; port null takes it off (the track has no instrument) → {track, port, channel}",
            has_selection,
            set_output
        ),
    ]
}

fn set_record_on_target(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "track.midi_record_on_target";
    let tracks = tracks_required(e, C, p)?;
    if let Some(t) = tracks.iter().filter_map(|t| e.session().track(*t)).find(|t| !t.kind.is_midi()) {
        return Err(bad(C, format!("`{}` is not an instrument or MIDI track", t.name)));
    }
    let now = tracks.first().and_then(|t| e.session().track(*t)).is_some_and(|t| t.midi_record_on_target);
    let value = match p.get("value") {
        None | Some(Value::Null) => !now,
        Some(v) => v.as_bool().ok_or_else(|| bad(C, "`value` is true or false"))?,
    };
    for t in &tracks {
        if let Some(tr) = e.session_mut().track_mut(*t) {
            tr.midi_record_on_target = value;
        }
    }
    Ok(json!({"value": value}))
}

/// The track a MIDI to Track instrument plays (`None`: the instrument is something else).
pub fn target_track(ins: &Insert) -> Option<TrackId> {
    track_midi::target_of(ins)
}

/// Whether giving track `t` the instrument `ins` would feed a track into itself.
fn closes_loop(e: &Engine, t: TrackId, ins: &Insert) -> bool {
    let mut after = e.session().clone();
    if let Some(tr) = after.track_mut(t) {
        tr.instrument = Some(ins.clone());
    }
    // (Routes resolve in track order: the one that closes the loop may be another's.)
    let cycles = |s: &soundcraft_model::Session| {
        soundcraft_mix::module_input_routes(s).iter().filter(|r| r.result == Err(soundcraft_mix::SidechainProblem::Cycle)).count()
    };
    cycles(&after) > cycles(e.session())
}

/// A `[lo, hi]` range from `v`, each end within `lo..=hi`, put in order.
fn range_param(v: Option<&Value>, lo: u8, hi: u8, key: &str) -> Result<(u8, u8)> {
    const C: &str = "mix.midi_layers";
    let Some(v) = v else { return Ok((lo, hi)) };
    let err = || bad(C, format!("`{key}` is [low, high], each {lo}-{hi}"));
    let arr = v.as_array().filter(|a| a.len() == 2).ok_or_else(err)?;
    let n = |i: usize| arr.get(i).and_then(Value::as_u64).filter(|x| (u64::from(lo)..=u64::from(hi)).contains(x)).map(|x| x as u8).ok_or_else(err);
    let (a, b) = (n(0)?, n(1)?);
    Ok((a.min(b), a.max(b)))
}

fn set_layers(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.midi_layers";
    let t = tracks_required(e, C, p)?.first().copied().ok_or_else(|| bad(C, "no track"))?;
    let list = p.get("layers").and_then(Value::as_array).ok_or_else(|| bad(C, "`layers` (a list, [] for none) required"))?;
    if list.len() > track_midi::MAX_ZONES {
        return Err(bad(C, format!("a track plays at most {} layers", track_midi::MAX_ZONES)));
    }
    let track = e.session().track(t).ok_or_else(|| bad(C, "no such track"))?;
    if !track.kind.is_midi() {
        return Err(bad(C, "only instrument and MIDI tracks send MIDI"));
    }
    let kept = track.instrument.as_ref().filter(|i| i.plugin == track_midi::ID).and_then(|i| i.params.get("channel").copied()).unwrap_or(0.0);
    let mut zones = Vec::new();
    for layer in list {
        let to = track_param(e, C, layer, "target")?.ok_or_else(|| bad(C, "each layer needs a `target` track"))?;
        let tt = e.session().track(to).ok_or_else(|| bad(C, "no such target track"))?;
        if to == t || !tt.kind.is_midi() {
            return Err(bad(C, "each target must be another instrument or MIDI track"));
        }
        let keys = range_param(layer.get("keys"), 0, 127, "keys")?;
        let vel = range_param(layer.get("velocities"), 1, 127, "velocities")?;
        zones.push(track_midi::Zone { track: to, keys, vel });
    }
    let ins = if zones.is_empty() {
        None
    } else {
        let mut ins = Insert::new(track_midi::ID);
        ins.params.insert("channel".into(), kept);
        ins.set_state_bytes(track_midi::zones_text(&zones).as_bytes());
        if closes_loop(e, t, &ins) {
            return Err(bad(C, "That would feed a track into itself"));
        }
        Some(ins)
    };
    if e.session().track(t).map(|tr| &tr.instrument) == Some(&ins) {
        return Ok(json!({"track": t, "layers": list}));
    }
    if let Some(tr) = e.session_mut().track_mut(t) {
        tr.instrument = ins;
    }
    let out: Vec<Value> = zones.iter().map(|z| json!({"target": z.track, "keys": [z.keys.0, z.keys.1], "velocities": [z.vel.0, z.vel.1]})).collect();
    Ok(json!({"track": t, "layers": out}))
}

/// The layers of a MIDI to Track instrument (one zone for a plain route).
pub fn layers(ins: &Insert) -> Vec<track_midi::Zone> {
    track_midi::zones_of(ins)
}

fn set_to_track(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.midi_to_track";
    let t = tracks_required(e, C, p)?.first().copied().ok_or_else(|| bad(C, "no track"))?;
    let target = match p.get("target") {
        Some(Value::Null) => None,
        Some(_) => Some(track_param(e, C, p, "target")?.ok_or_else(|| bad(C, "no such target track"))?),
        None => return Err(bad(C, "`target` (a track, or null) required")),
    };
    let channel = match p.get("channel") {
        None => None,
        Some(v) => Some(v.as_u64().filter(|c| *c <= 16).ok_or_else(|| bad(C, "`channel` is 0 (as played) to 16"))? as f32),
    };
    let track = e.session().track(t).ok_or_else(|| bad(C, "no such track"))?;
    if !track.kind.is_midi() {
        return Err(bad(C, "only instrument and MIDI tracks send MIDI"));
    }
    let kept = track.instrument.as_ref().filter(|i| i.plugin == track_midi::ID).and_then(|i| i.params.get("channel").copied());
    let channel = channel.or(kept).unwrap_or(0.0);
    let ins = match target {
        None => None,
        Some(to) => {
            let tt = e.session().track(to).ok_or_else(|| bad(C, "no such target track"))?;
            if !tt.kind.is_midi() {
                return Err(bad(C, "the target must be an instrument or MIDI track"));
            }
            let mut ins = Insert::new(track_midi::ID);
            ins.params.insert("channel".into(), channel);
            ins.set_state_bytes(to.0.to_string().as_bytes());
            // Refused when it would feed a track into itself.
            if closes_loop(e, t, &ins) {
                return Err(bad(C, "That would feed a track into itself"));
            }
            Some(ins)
        }
    };
    if let Some(tr) = e.session_mut().track_mut(t) {
        tr.instrument = ins;
    }
    Ok(json!({"track": t, "target": target, "channel": channel as u8}))
}

/// The port an External MIDI instrument sends to (`None`: the instrument is something else).
pub fn output_port(ins: &Insert) -> Option<String> {
    (ins.plugin == external_midi::ID).then(|| ins.state_bytes().and_then(|b| String::from_utf8(b).ok()).unwrap_or_default())
}

/// A fresh `state_load` for each port set.
static NEXT_LOAD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn set_output(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.midi_output";
    let t = tracks_required(e, C, p)?.first().copied().ok_or_else(|| bad(C, "no track"))?;
    let port = match p.get("port") {
        Some(Value::Null) => None,
        Some(Value::String(s)) if !s.trim().is_empty() && s.len() <= 256 => Some(s.clone()),
        _ => return Err(bad(C, "`port` (a MIDI output port's name, or null) required")),
    };
    let channel = match p.get("channel") {
        None => None,
        Some(v) => Some(v.as_u64().filter(|c| *c <= 16).ok_or_else(|| bad(C, "`channel` is 0 (as played) to 16"))? as f32),
    };
    let track = e.session().track(t).ok_or_else(|| bad(C, "no such track"))?;
    if !track.kind.is_midi() {
        return Err(bad(C, "only instrument and MIDI tracks send MIDI"));
    }
    let kept = track.instrument.as_ref().filter(|i| i.plugin == external_midi::ID).and_then(|i| i.params.get("channel").copied());
    let channel = channel.or(kept).unwrap_or(0.0);
    let ins = port.as_ref().map(|name| {
        let mut ins = Insert::new(external_midi::ID);
        ins.params.insert("channel".into(), channel);
        ins.set_state_bytes(name.as_bytes());
        // A new port is a new instance (the running one lets go of its notes on the old port).
        ins.state_load = NEXT_LOAD.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        ins
    });
    if let Some(tr) = e.session_mut().track_mut(t) {
        tr.instrument = ins;
    }
    Ok(json!({"track": t, "port": port, "channel": channel as u8}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::{Value, json};

    #[test]
    fn midi_tracks_play_an_instrument_elsewhere_and_loops_are_refused() {
        let mut e = Engine::default();
        let new = |e: &mut Engine, kind: &str| e.execute("track.new", &json!({"kind": kind})).unwrap()["tracks"][0].as_u64().unwrap();
        let synth = new(&mut e, "instrument");
        let (a, b) = (new(&mut e, "midi"), new(&mut e, "midi"));
        let out = e.execute("mix.midi_to_track", &json!({"track": a, "target": synth, "channel": 1})).unwrap();
        assert_eq!((out["target"].as_u64(), out["channel"].as_u64()), (Some(synth), Some(1)));
        e.execute("mix.midi_to_track", &json!({"track": b, "target": synth, "channel": 2})).unwrap();
        let ins = |e: &Engine, t: u64| e.session().track(soundcraft_model::TrackId(t)).unwrap().instrument.clone().unwrap();
        assert_eq!(super::target_track(&ins(&e, b)), Some(soundcraft_model::TrackId(synth)));
        assert_eq!(ins(&e, b).params.get("channel"), Some(&2.0));
        // The synth playing track a's instrument would feed a into itself.
        let err = e.execute("mix.midi_to_track", &json!({"track": synth, "target": a})).unwrap_err().to_string();
        assert!(err.contains("into itself"), "{err}");
        assert!(e.execute("mix.midi_to_track", &json!({"track": a, "target": a})).is_err());
        let audio = new(&mut e, "audio");
        assert!(e.execute("mix.midi_to_track", &json!({"track": a, "target": audio})).is_err());
        // It saves and opens with the session; null takes it off, undo puts it back.
        let json = serde_json::to_string(e.session()).unwrap();
        let back: soundcraft_model::Session = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.track(soundcraft_model::TrackId(a)).and_then(|t| t.instrument.as_ref()).and_then(super::target_track),
            Some(soundcraft_model::TrackId(synth))
        );
        e.execute("mix.midi_to_track", &json!({"track": a, "target": null})).unwrap();
        assert!(e.session().track(soundcraft_model::TrackId(a)).unwrap().instrument.is_none());
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(super::target_track(&ins(&e, a)), Some(soundcraft_model::TrackId(synth)));
    }

    #[test]
    fn a_track_layers_and_splits_several_instruments_and_undo_takes_it_back() {
        let mut e = Engine::default();
        let new = |e: &mut Engine, kind: &str| e.execute("track.new", &json!({"kind": kind})).unwrap()["tracks"][0].as_u64().unwrap();
        let (bass, piano, pad) = (new(&mut e, "instrument"), new(&mut e, "instrument"), new(&mut e, "instrument"));
        let keys = new(&mut e, "midi");
        let out = e
            .execute(
                "mix.midi_layers",
                &json!({"track": keys, "layers": [
                    {"target": bass, "keys": [0, 59]},
                    {"target": piano, "keys": [60, 127]},
                    {"target": pad, "keys": [127, 60], "velocities": [100, 127]},
                ]}),
            )
            .unwrap();
        assert_eq!(out["layers"][2]["keys"], json!([60, 127]), "ranges are put in order");
        let ins = e.session().track(soundcraft_model::TrackId(keys)).unwrap().instrument.clone().unwrap();
        let zones = super::layers(&ins);
        assert_eq!(zones.len(), 3);
        assert_eq!((zones[2].keys, zones[2].vel), ((60, 127), (100, 127)));
        // A layer that would feed back is refused; so are too many layers and bad ranges.
        assert!(e.execute("mix.midi_layers", &json!({"track": bass, "layers": [{"target": keys}]})).is_err());
        let five: Vec<Value> = (0..5).map(|_| json!({"target": bass})).collect();
        assert!(e.execute("mix.midi_layers", &json!({"track": keys, "layers": five})).is_err());
        assert!(e.execute("mix.midi_layers", &json!({"track": keys, "layers": [{"target": bass, "keys": [0, 200]}]})).is_err());
        assert!(e.execute("mix.midi_layers", &json!({"track": keys, "layers": [{"target": bass, "velocities": [0, 10]}]})).is_err());
        assert!(e.execute("mix.midi_layers", &json!({"track": keys, "layers": [{"target": keys}]})).is_err());
        // Saved and opened; [] takes them off; undo puts them back.
        let json = serde_json::to_string(e.session()).unwrap();
        let back: soundcraft_model::Session = serde_json::from_str(&json).unwrap();
        assert_eq!(super::layers(back.track(soundcraft_model::TrackId(keys)).unwrap().instrument.as_ref().unwrap()), zones);
        e.execute("mix.midi_layers", &json!({"track": keys, "layers": []})).unwrap();
        assert!(e.session().track(soundcraft_model::TrackId(keys)).unwrap().instrument.is_none());
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(super::layers(e.session().track(soundcraft_model::TrackId(keys)).unwrap().instrument.as_ref().unwrap()), zones);
    }

    #[test]
    fn a_track_sends_to_a_port_by_name_and_undo_puts_its_instrument_back() {
        let mut e = Engine::default();
        let t = e.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        let before = e.session().tracks[0].instrument.clone();
        let out = e.execute("mix.midi_output", &json!({"track": t, "port": "Juno USB", "channel": 3})).unwrap();
        assert_eq!((out["port"].as_str(), out["channel"].as_u64()), (Some("Juno USB"), Some(3)));
        let ins = e.session().tracks[0].instrument.clone().unwrap();
        assert_eq!(super::output_port(&ins).as_deref(), Some("Juno USB"));
        assert_eq!(ins.params.get("channel"), Some(&3.0));
        // Another port keeps the channel.
        e.execute("mix.midi_output", &json!({"track": t, "port": "Moog"})).unwrap();
        assert_eq!(e.session().tracks[0].instrument.as_ref().unwrap().params.get("channel"), Some(&3.0));
        e.execute("edit.undo", &json!({})).unwrap();
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().tracks[0].instrument, before);
        assert!(e.execute("mix.midi_output", &json!({"track": t, "port": "X", "channel": 17})).is_err());
        assert!(e.execute("mix.midi_output", &json!({"track": t})).is_err());
        let audio = e.execute("track.new", &json!({"kind": "audio"})).unwrap()["tracks"][0].as_u64().unwrap();
        assert!(e.execute("mix.midi_output", &json!({"track": audio, "port": "X"})).is_err());
        // It saves and opens as any instrument does.
        e.execute("mix.midi_output", &json!({"track": t, "port": "Juno USB"})).unwrap();
        let json = serde_json::to_string(e.session()).unwrap();
        let back: soundcraft_model::Session = serde_json::from_str(&json).unwrap();
        let ins = back.track(soundcraft_model::TrackId(t)).unwrap().instrument.clone().unwrap();
        assert_eq!(super::output_port(&ins).as_deref(), Some("Juno USB"));
    }
}
