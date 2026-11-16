//! MIDI learn: the session's controller mappings (`Session::midi_maps`) and what a mapped
//! controller does.
//!
//! A mapping binds a CC or a note from an input port (or any port) to a track's volume, pan, a
//! send's level or pan, mute, or a plug-in parameter, over part of its range, maybe inverted.
//! The mappings are saved with the session because they name its tracks and plug-ins. The host
//! learns them (right-click a control › Learn MIDI Controller, then move a knob), keeps the mapped
//! messages out of takes and Thru, and calls [`apply`] for each one; a move goes through the
//! control's own command, so it is undone and written to automation as a move of the on-screen
//! control is (one undo step per gesture).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{AutoParam, MidiMap, MidiSource, MidiSourceKind, MidiTarget, TrackId, TransportAction};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "midi.map_add",
            "Add MIDI Mapping",
            [],
            None,
            "{source: {port?, kind: cc|note, channel: 0-15, number: 0-127}, track, param: volume|pan|send a level|… |plugin id, min?: 0..1, max?: 0..1, invert?: bool} — a controller moves the parameter (replaces a mapping of the same source) → {index}",
            always,
            add
        ),
        cmd!("midi.map_remove", "Remove MIDI Mapping", [], None, "{index}", always, remove),
        cmd!("midi.map_set", "Set MIDI Mapping Range", [], None, "{index, min?: 0..1, max?: 0..1, invert?: bool}", always, set),
        cmd!(query "midi.maps", "MIDI Mappings", [], None, "{} → [{source, target, min, max, invert, label}]", always, |e, _| {
            let s = e.session();
            Ok(json!(s.midi_maps.iter().map(|m| json!({"source": m.source, "target": m.target, "min": m.min, "max": m.max, "invert": m.invert, "label": target_label(s, &m.target)})).collect::<Vec<_>>()))
        }),
    ]
}

/// How the mapping list names a target: "Kick volume", "Bass CleanQ gain", "Play".
pub fn target_label(s: &soundcraft_model::Session, t: &MidiTarget) -> String {
    match t {
        MidiTarget::Transport(a) => a.label().to_string(),
        MidiTarget::Param { track, param } => {
            let name = s.track(*track).map_or("(deleted track)", |t| t.name.as_str());
            let what = match param {
                AutoParam::Plugin { module, param } => {
                    let plugin = s.track(*track).and_then(|t| t.mixer.module(*module)).map(|i| i.plugin.clone()).unwrap_or_default();
                    let pname = super::clap::plugin_info(&plugin)
                        .and_then(|i| i.params.iter().find(|p| p.id == param.as_str()))
                        .map_or(param.clone(), |p| p.name.to_string());
                    let short = super::clap::plugin_info(&plugin).map_or(plugin.as_str(), |i| i.short_name);
                    format!("{short} {pname}")
                }
                p => p.label(),
            };
            format!("{name} {what}")
        }
    }
}

fn source_of(p: &Value, c: &str) -> Result<MidiSource> {
    let v = p.get("source").ok_or_else(|| bad(c, "`source` required"))?;
    let kind = match v.get("kind").and_then(Value::as_str) {
        Some("cc") => MidiSourceKind::Cc,
        Some("note") => MidiSourceKind::Note,
        _ => return Err(bad(c, "`source.kind` is cc or note")),
    };
    let n = |k: &str, max: u64| {
        v.get(k).and_then(Value::as_u64).filter(|x| *x <= max).map(|x| x as u8).ok_or_else(|| bad(c, format!("`source.{k}` is 0 to {max}")))
    };
    let port = v.get("port").and_then(Value::as_str).filter(|s| !s.is_empty() && s.len() <= 256).map(str::to_string);
    Ok(MidiSource { port, kind, channel: n("channel", 15)?, number: n("number", 127)? })
}

fn unit(p: &Value, k: &str, c: &str) -> Result<Option<f32>> {
    match p.get(k) {
        None => Ok(None),
        Some(v) => v.as_f64().filter(|x| (0.0..=1.0).contains(x)).map(|x| Some(x as f32)).ok_or_else(|| bad(c, format!("`{k}` is 0 to 1"))),
    }
}

fn add(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.map_add";
    let source = source_of(p, C)?;
    let t = tracks_required(e, C, p)?.first().copied().ok_or_else(|| bad(C, "`track` required"))?;
    let lanes = super::mix::lane_params(e, &[t], &json!({"param": str_param(p, "param").unwrap_or("")}), C)?;
    let param = lanes.into_iter().next().map(|(_, p)| p).ok_or_else(|| bad(C, "`param` required"))?;
    let mut m = MidiMap::new(source, MidiTarget::Param { track: t, param });
    m.min = unit(p, "min", C)?.unwrap_or(0.0);
    m.max = unit(p, "max", C)?.unwrap_or(1.0);
    m.invert = p.get("invert").and_then(Value::as_bool).unwrap_or(false);
    let maps = &mut e.session_mut().midi_maps;
    maps.retain(|x| x.source != m.source);
    maps.push(m);
    Ok(json!({"index": maps.len() - 1}))
}

fn index(e: &Engine, p: &Value, c: &str) -> Result<usize> {
    p.get("index")
        .and_then(Value::as_u64)
        .map(|i| i as usize)
        .filter(|i| *i < e.session().midi_maps.len())
        .ok_or_else(|| bad(c, "no mapping at `index`"))
}

fn remove(e: &mut Engine, p: &Value) -> Result<Value> {
    let i = index(e, p, "midi.map_remove")?;
    e.session_mut().midi_maps.remove(i);
    Ok(json!({}))
}

fn set(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.map_set";
    let i = index(e, p, C)?;
    let (min, max) = (unit(p, "min", C)?, unit(p, "max", C)?);
    let invert = p.get("invert").and_then(Value::as_bool);
    if let Some(m) = e.session_mut().midi_maps.get_mut(i) {
        m.min = min.unwrap_or(m.min);
        m.max = max.unwrap_or(m.max);
        m.invert = invert.unwrap_or(m.invert);
    }
    Ok(json!({}))
}

/// A mapped controller sent `value` (0-127; a note-off is 0): the parameter moves through its
/// own command, merged into one undo step while the controller keeps moving it. Transport
/// targets are the host's ([`transport_command`]).
pub fn apply(e: &mut Engine, map: &MidiMap, value: u8) -> Result<()> {
    let MidiTarget::Param { track, param } = &map.target else { return Ok(()) };
    let (t, pos) = (*track, map.position(value));
    let key = format!("learn:{}:{}", t.0, param.id());
    let db = || soundcraft_model::fader_pos_to_db(pos).clamp(-144.0, 12.0);
    let (id, params) = match param {
        AutoParam::Volume => ("mix.volume", json!({"track": t.0, "db": db()})),
        AutoParam::Pan(i) => ("mix.pan", json!({"track": t.0, "pan": pos * 2.0 - 1.0, "index": i})),
        AutoParam::Mute => ("mix.mute", json!({"tracks": [t.0], "value": pos >= 0.5})),
        AutoParam::SendLevel(i) => ("mix.send_level", json!({"track": t.0, "slot": i, "db": db()})),
        AutoParam::SendPan(i) => ("mix.send_level", json!({"track": t.0, "slot": i, "pan": pos * 2.0 - 1.0})),
        AutoParam::SendMute(i) => ("mix.send_level", json!({"track": t.0, "slot": i, "mute": pos >= 0.5})),
        AutoParam::Trim => ("mix.trim", json!({"tracks": [t.0], "db": pos * 48.0 - 24.0})),
        AutoParam::Plugin { module, param } => {
            let plugin = e
                .session()
                .track(t)
                .and_then(|tr| tr.mixer.module(*module))
                .map(|i| i.plugin.clone())
                .ok_or_else(|| bad("midi learn", "the plug-in is gone"))?;
            let info = super::clap::plugin_info(&plugin)
                .and_then(|i| i.params.iter().find(|p| p.id == param.as_str()))
                .ok_or_else(|| bad("midi learn", "no such parameter"))?;
            ("mix.module_param", json!({"track": t.0, "module": module.0, "param": param, "value": info.from_normalized(pos)}))
        }
    };
    e.execute_merged(id, &params, &key).map(|_| ())
}

/// The command a transport mapping runs.
pub fn transport_command(a: TransportAction) -> &'static str {
    match a {
        TransportAction::Play => "transport.play",
        TransportAction::Stop => "transport.stop",
        TransportAction::PlayStop => "transport.toggle",
        TransportAction::Record | TransportAction::Punch => "transport.record",
        TransportAction::ReturnToZero => "transport.rtz",
        TransportAction::Loop => "options.loop_playback",
    }
}

/// The transport command an MMC message (`F0 7F <device> 06 <command> F7`) asks for: stop,
/// play, deferred play, fast forward, rewind, record strobe (punch in), record exit, pause.
pub fn mmc_command(sysex: &[u8]) -> Option<&'static str> {
    match sysex {
        [0xF0, 0x7F, _, 0x06, cmd, ..] => match cmd {
            0x01 => Some("transport.stop"),
            0x02 | 0x03 => Some("transport.play"),
            0x04 => Some("transport.fast_forward"),
            0x05 => Some("transport.rewind"),
            0x06 | 0x07 => Some("transport.record"),
            0x09 => Some("transport.pause"),
            _ => None,
        },
        _ => None,
    }
}

/// The session's mappings a message from `port` moves (by index).
pub fn matching(s: &soundcraft_model::Session, port: &str, msg: [u8; 3]) -> Vec<usize> {
    s.midi_maps.iter().enumerate().filter(|(_, m)| m.source.matches(port, msg)).map(|(i, _)| i).collect()
}

/// A track the mapping moves (for a host that holds the control touched).
pub fn mapped_track(m: &MidiMap) -> Option<TrackId> {
    match m.target {
        MidiTarget::Param { track, .. } => Some(track),
        MidiTarget::Transport(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;

    #[test]
    fn a_learnt_knob_moves_its_parameter_over_its_range_and_undoes_as_one_step() {
        let mut e = Engine::default();
        let t = e.execute("track.new", &json!({"kind": "audio", "name": "Kick"})).unwrap()["tracks"][0].as_u64().unwrap();
        let src = json!({"kind": "cc", "channel": 0, "number": 7, "port": "Keys"});
        e.execute("midi.map_add", &json!({"source": src, "track": t, "param": "pan"})).unwrap();
        e.execute("midi.map_add", &json!({"source": {"kind": "cc", "channel": 0, "number": 8}, "track": t, "param": "volume", "invert": true}))
            .unwrap();
        assert_eq!(e.session().midi_maps.len(), 2);
        let maps = e.execute("midi.maps", &json!({})).unwrap();
        assert_eq!(maps[0]["label"], "Kick pan");
        let ix = super::matching(e.session(), "Keys", [0xB0, 7, 127]);
        assert_eq!(ix, vec![0]);
        assert!(super::matching(e.session(), "Other", [0xB0, 7, 127]).is_empty(), "port-bound");
        let m = e.session().midi_maps[0].clone();
        for v in [64, 100, 127] {
            super::apply(&mut e, &m, v).unwrap();
        }
        assert_eq!(e.session().track(soundcraft_model::TrackId(t)).unwrap().mixer.pan[0], 1.0);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().track(soundcraft_model::TrackId(t)).unwrap().mixer.pan[0], 0.0, "the knob's moves are one step");
        // Inverted, a range: the bottom of the knob is the top of the range.
        e.execute("midi.map_set", &json!({"index": 1, "min": 0.0, "max": 0.5})).unwrap();
        let m = e.session().midi_maps[1].clone();
        super::apply(&mut e, &m, 0).unwrap();
        let want = soundcraft_model::fader_pos_to_db(0.5);
        assert!((e.session().track(soundcraft_model::TrackId(t)).unwrap().mixer.volume_db - want).abs() < 0.01);
        e.execute("midi.map_remove", &json!({"index": 0})).unwrap();
        assert_eq!(e.session().midi_maps.len(), 1);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().midi_maps.len(), 2, "removing a mapping undoes");
        assert!(e.execute("midi.map_add", &json!({"source": {"kind": "pb", "channel": 0, "number": 0}, "track": t, "param": "pan"})).is_err());
        // Saved with the session.
        let back: soundcraft_model::Session = serde_json::from_str(&serde_json::to_string(e.session()).unwrap()).unwrap();
        assert_eq!(back.midi_maps, e.session().midi_maps);
    }

    #[test]
    fn mmc_runs_the_transport() {
        assert_eq!(super::mmc_command(&[0xF0, 0x7F, 0x7F, 0x06, 0x02, 0xF7]), Some("transport.play"));
        assert_eq!(super::mmc_command(&[0xF0, 0x7F, 0x00, 0x06, 0x01, 0xF7]), Some("transport.stop"));
        assert_eq!(super::mmc_command(&[0xF0, 0x7E, 0x00, 0x06, 0x01, 0xF7]), None);
    }
}
