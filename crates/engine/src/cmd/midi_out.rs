//! A track's MIDI output to hardware (`mix.midi_output`): an instrument or MIDI track plays a
//! hardware synth (or any device on a MIDI output port) through the External MIDI instrument
//! (`soundcraft_mix::external_midi`). The port is kept by name in the instrument's state, the
//! channel (0: as played, 1-16) as its parameter, so the session opens with it on any computer
//! that has a port of that name.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_mix::external_midi;
use soundcraft_model::Insert;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "mix.midi_output",
        "MIDI Output",
        [],
        None,
        "{track?, port: name | null, channel?: 0..16} — an instrument or MIDI track sends what it plays to a hardware MIDI output port (its instrument becomes External MIDI); channel 0 keeps each event's own; port null takes it off (the track has no instrument) → {track, port, channel}",
        has_selection,
        set_output
    )]
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
    use serde_json::json;

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
