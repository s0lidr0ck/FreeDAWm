//! The control room (Window › Control Room, [`soundcraft_model::ControlRoom`]): monitor sets with
//! level, dim, mono and mute; the listen bus of AFL/PFL solo; cue mixes fed by pre-fader sends to
//! hardware outputs; a talkback microphone into the cues. Saved with the session. The moment-to-
//! moment controls (level, dim, mono, mute, the set in use, talkback) are not undo steps; making
//! sets and cues, and the cue sends, are.

use super::*;
use crate::cmd;
use serde_json::{Value, json};
use soundcraft_model::{ControlRoom, CueMix, MAX_CUES, MAX_MONITOR_SETS, MonitorSet, Route, SendSlot, TrackKind};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "monitor.control_room",
            "Control Room",
            [],
            None,
            "{on?: bool} — gives the session a control room (or takes it away); toggles when omitted → {on}",
            always,
            enable
        ),
        cmd!(noundo "monitor.level", "Monitor Level", [], None, "{db: number} — the monitor set's level → {db}", always, level),
        cmd!(noundo "monitor.dim", "Dim", [], None, "{on?: bool, db?: number} — dim the monitor (toggles when `on` is omitted); `db` sets how far → {on, db}", always, dim),
        cmd!(noundo "monitor.mono", "Mono", [], None, "{on?: bool} — the monitor in mono; toggles when omitted → {on}", always, mono),
        cmd!(noundo "monitor.mute", "Mute Monitor", [], None, "{on?: bool} — the monitor speakers muted; toggles when omitted → {on}", always, mute),
        cmd!(noundo "monitor.listen_level", "Listen Level", [], None, "{db: number} — the listen bus (AFL/PFL solo) level → {db}", always, listen_level),
        cmd!(noundo "monitor.select_set", "Select Monitor Set", [], None, "{set: index} — the speakers the monitor plays on → {set}", always, select_set),
        cmd!(
            "monitor.set",
            "Monitor Set",
            [],
            None,
            "{set?: index — omitted: a new one, name?, output?: output path name ('' the main output), level_db?, trim_db?} → {set}",
            always,
            set
        ),
        cmd!("monitor.set_remove", "Remove Monitor Set", [], None, "{set: index} → {}", always, set_remove),
        cmd!(
            "monitor.cue",
            "Cue Mix",
            [],
            None,
            "{cue?: index — omitted: a new one, name?, output?: output path name, level_db?, mute?, talkback?: bool} → {cue}",
            always,
            cue
        ),
        cmd!("monitor.cue_remove", "Remove Cue Mix", [], None, "{cue: index} — the cue and its sends → {removed_sends}", always, cue_remove),
        cmd!(
            "monitor.cue_sends",
            "Cue Sends",
            [],
            None,
            "{cue: index, tracks?: [id|name] (default: every audio, instrument and aux track), copy_fader?: bool} — a pre-fader send to the cue's output on each track that has none → {added}",
            always,
            cue_sends
        ),
        cmd!(
            "monitor.talkback_setup",
            "Talkback Setup",
            [],
            None,
            "{input?: input path name, level_db?, dims?: bool} → {input}",
            always,
            talkback_setup
        ),
        cmd!(noundo "monitor.talkback", "Talkback", [], None, "{on?: bool} — talk to the cues (hold: on when pressed, off when let go); toggles when omitted → {on}", always, talkback),
    ]
}

/// The session's control room, made when a control is first used.
fn room(e: &mut Engine) -> &mut ControlRoom {
    e.session_mut().control_room.get_or_insert_with(ControlRoom::default)
}

/// `on`, or the opposite of `now` when it is not given.
fn toggle(p: &Value, now: bool) -> bool {
    bool_or(p, "on", !now)
}

fn db(p: &Value, cmd: &str, key: &str, lo: f32, hi: f32) -> Result<f32> {
    p.get(key)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
        .map(|v| (v as f32).clamp(lo, hi))
        .ok_or_else(|| bad(cmd, format!("`{key}` (dB) required")))
}

fn index(p: &Value, cmd: &str, key: &str, len: usize) -> Result<usize> {
    p.get(key)
        .and_then(Value::as_u64)
        .and_then(|i| usize::try_from(i).ok())
        .filter(|i| *i < len)
        .ok_or_else(|| bad(cmd, format!("`{key}` names none")))
}

/// An output path's name as the session spells it; empty stays empty (the main output where
/// allowed).
fn output_name(e: &Engine, cmd: &str, name: &str, main_ok: bool) -> Result<String> {
    if name.is_empty() && main_ok {
        return Ok(String::new());
    }
    let s = e.session();
    let i = s.output_index(name).ok_or_else(|| bad(cmd, format!("no output named `{name}` (Setup › I/O)")))?;
    if i == 0 && !main_ok {
        return Err(bad(cmd, "a cue needs its own output, not the main one"));
    }
    Ok(s.outputs.get(i).map(|o| o.name.clone()).unwrap_or_default())
}

fn enable(e: &mut Engine, p: &Value) -> Result<Value> {
    let on = toggle(p, e.session().control_room.is_some());
    let s = e.session_mut();
    if on {
        s.control_room.get_or_insert_with(ControlRoom::default);
    } else {
        s.control_room = None;
    }
    Ok(json!({"on": on}))
}

fn level(e: &mut Engine, p: &Value) -> Result<Value> {
    let v = db(p, "monitor.level", "db", -144.0, 12.0)?;
    let r = room(e);
    let at = r.active_set.min(r.sets.len().saturating_sub(1));
    if let Some(set) = r.sets.get_mut(at) {
        set.level_db = v;
    }
    Ok(json!({"db": v}))
}

fn dim(e: &mut Engine, p: &Value) -> Result<Value> {
    let lvl = p.get("db").map(|_| db(p, "monitor.dim", "db", -60.0, 0.0)).transpose()?;
    let r = room(e);
    if let Some(v) = lvl {
        r.dim_db = v;
    }
    if p.get("on").is_some() || lvl.is_none() {
        r.dim = toggle(p, r.dim);
    }
    Ok(json!({"on": r.dim, "db": r.dim_db}))
}

fn mono(e: &mut Engine, p: &Value) -> Result<Value> {
    let r = room(e);
    r.mono = toggle(p, r.mono);
    Ok(json!({"on": r.mono}))
}

fn mute(e: &mut Engine, p: &Value) -> Result<Value> {
    let r = room(e);
    r.mute = toggle(p, r.mute);
    Ok(json!({"on": r.mute}))
}

fn listen_level(e: &mut Engine, p: &Value) -> Result<Value> {
    let v = db(p, "monitor.listen_level", "db", -144.0, 12.0)?;
    room(e).listen_db = v;
    Ok(json!({"db": v}))
}

fn select_set(e: &mut Engine, p: &Value) -> Result<Value> {
    let n = room(e).sets.len();
    let i = index(p, "monitor.select_set", "set", n)?;
    room(e).active_set = i;
    Ok(json!({"set": i}))
}

fn set(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "monitor.set";
    let output = str_param(p, "output").map(|o| output_name(e, C, o, true)).transpose()?;
    let n = room(e).sets.len();
    let i = match p.get("set") {
        Some(_) => index(p, C, "set", n)?,
        None if n >= MAX_MONITOR_SETS => return Err(bad(C, format!("a control room keeps {MAX_MONITOR_SETS} monitor sets"))),
        None => {
            room(e).sets.push(MonitorSet { name: format!("Set {}", n + 1), output: String::new(), level_db: 0.0, trim_db: 0.0 });
            n
        }
    };
    let level = p.get("level_db").map(|_| db(p, C, "level_db", -144.0, 12.0)).transpose()?;
    let trim = p.get("trim_db").map(|_| db(p, C, "trim_db", -24.0, 24.0)).transpose()?;
    let r = room(e);
    let Some(x) = r.sets.get_mut(i) else { return Err(bad(C, "no such set")) };
    if let Some(name) = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        x.name = name.to_string();
    }
    if let Some(o) = output {
        x.output = o;
    }
    if let Some(v) = level {
        x.level_db = v;
    }
    if let Some(v) = trim {
        x.trim_db = v;
    }
    Ok(json!({"set": i}))
}

fn set_remove(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "monitor.set_remove";
    let n = room(e).sets.len();
    let i = index(p, C, "set", n)?;
    if n <= 1 {
        return Err(bad(C, "a control room keeps at least one monitor set"));
    }
    let r = room(e);
    r.sets.remove(i);
    if r.active_set > i || r.active_set >= r.sets.len() {
        r.active_set = r.active_set.saturating_sub(1);
    }
    Ok(json!({}))
}

fn cue(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "monitor.cue";
    let output = str_param(p, "output").map(|o| output_name(e, C, o, false)).transpose()?;
    let n = room(e).cues.len();
    let i = match p.get("cue") {
        Some(_) => index(p, C, "cue", n)?,
        None if n >= MAX_CUES => return Err(bad(C, format!("a control room keeps {MAX_CUES} cue mixes"))),
        None => {
            let out = output.clone().ok_or_else(|| bad(C, "a new cue needs an `output`"))?;
            room(e).cues.push(CueMix { name: format!("Cue {}", n + 1), output: out, level_db: 0.0, mute: false, talkback: true });
            n
        }
    };
    let level = p.get("level_db").map(|_| db(p, C, "level_db", -144.0, 12.0)).transpose()?;
    let r = room(e);
    let Some(x) = r.cues.get_mut(i) else { return Err(bad(C, "no such cue")) };
    if let Some(name) = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        x.name = name.to_string();
    }
    if let Some(o) = output {
        x.output = o;
    }
    if let Some(v) = level {
        x.level_db = v;
    }
    x.mute = bool_or(p, "mute", x.mute);
    x.talkback = bool_or(p, "talkback", x.talkback);
    Ok(json!({"cue": i}))
}

/// Whether `r` is the hardware output `out`.
fn is_output(r: &Route, out: &str) -> bool {
    matches!(r, Route::Hardware(h) if h.eq_ignore_ascii_case(out))
}

fn cue_remove(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "monitor.cue_remove";
    let n = room(e).cues.len();
    let i = index(p, C, "cue", n)?;
    let gone = room(e).cues.remove(i);
    // Its sends go with it, unless another cue plays on the same output.
    let shared = room(e).cues.iter().any(|c| c.output.eq_ignore_ascii_case(&gone.output));
    let mut removed = 0;
    if !shared {
        for t in &mut e.session_mut().tracks {
            for sl in t.mixer.sends.iter_mut() {
                if sl.as_ref().is_some_and(|x| is_output(&x.target, &gone.output)) {
                    *sl = None;
                    removed += 1;
                }
            }
        }
    }
    Ok(json!({"removed_sends": removed}))
}

fn cue_sends(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "monitor.cue_sends";
    let n = e.session().control_room.as_ref().map_or(0, |r| r.cues.len());
    let i = index(p, C, "cue", n)?;
    let out = e.session().control_room.as_ref().and_then(|r| r.cues.get(i)).map(|c| c.output.clone()).unwrap_or_default();
    let tracks: Vec<TrackId> = if p.get("tracks").is_some() || p.get("track").is_some() {
        tracks_param(e, C, p)?
    } else {
        e.session().tracks.iter().filter(|t| matches!(t.kind, TrackKind::Audio | TrackKind::Instrument | TrackKind::Aux)).map(|t| t.id).collect()
    };
    let copy = bool_or(p, "copy_fader", true);
    let default_db = mix::default_send_db(e.session());
    let mut added = Vec::new();
    for t in tracks {
        let Some(tr) = e.session_mut().track_mut(t) else { continue };
        if tr.mixer.sends.iter().flatten().any(|x| is_output(&x.target, &out)) {
            continue;
        }
        let Some(slot) = tr.mixer.sends.iter().position(Option::is_none) else { continue };
        let level_db = if copy { tr.mixer.volume_db.clamp(-144.0, 12.0) } else { default_db };
        if let Some(x) = tr.mixer.sends.get_mut(slot) {
            *x = Some(SendSlot { level_db, pre_fader: true, ..SendSlot::new(Route::Hardware(out.clone())) });
            added.push(json!({"track": t.0, "slot": slot}));
        }
    }
    Ok(json!({"added": added}))
}

fn talkback_setup(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "monitor.talkback_setup";
    if let Some(input) = str_param(p, "input")
        && !input.is_empty()
        && e.session().input_channels(input).is_none()
    {
        return Err(bad(C, format!("no input named `{input}` (Setup › I/O)")));
    }
    let level = p.get("level_db").map(|_| db(p, C, "level_db", -144.0, 12.0)).transpose()?;
    let tb = &mut room(e).talkback;
    if let Some(input) = str_param(p, "input") {
        tb.input = input.to_string();
    }
    if let Some(v) = level {
        tb.level_db = v;
    }
    tb.dims = bool_or(p, "dims", tb.dims);
    Ok(json!({"input": tb.input}))
}

fn talkback(e: &mut Engine, p: &Value) -> Result<Value> {
    let tb = &mut room(e).talkback;
    tb.on = toggle(p, tb.on);
    Ok(json!({"on": tb.on}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, OutputPath};

    fn engine() -> Engine {
        let mut e = Engine::default();
        let s = e.session_mut();
        s.outputs.push(OutputPath { name: "Phones A".into(), first_channel: 2, format: ChannelFormat::Stereo });
        s.outputs.push(OutputPath { name: "Small Speakers".into(), first_channel: 4, format: ChannelFormat::Stereo });
        e
    }

    #[test]
    fn the_control_room_is_made_by_commands_and_saved_with_the_session() {
        let mut e = engine();
        let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Stereo, None);
        e.session_mut().track_mut(t).unwrap().mixer.volume_db = -6.0;
        e.execute("monitor.control_room", &json!({"on": true})).unwrap();
        e.execute("monitor.set", &json!({"name": "Small", "output": "small speakers", "trim_db": -2})).unwrap();
        e.execute("monitor.select_set", &json!({"set": 1})).unwrap();
        e.execute("monitor.level", &json!({"db": -10})).unwrap();
        e.execute("monitor.dim", &json!({})).unwrap();
        e.execute("monitor.mono", &json!({"on": true})).unwrap();
        e.execute("monitor.cue", &json!({"output": "Phones A", "level_db": -3})).unwrap();
        assert!(e.execute("monitor.cue", &json!({"output": "Main"})).is_err(), "a cue needs its own output");
        let added = e.execute("monitor.cue_sends", &json!({"cue": 0})).unwrap();
        assert_eq!(added["added"].as_array().map(Vec::len), Some(1));
        let send = e.session().track(t).unwrap().mixer.sends[0].clone().unwrap();
        assert!(send.pre_fader && send.level_db == -6.0 && send.target == Route::Hardware("Phones A".into()), "pre-fader, from the fader");
        assert_eq!(e.execute("monitor.cue_sends", &json!({"cue": 0})).unwrap()["added"], json!([]), "one per track");
        e.execute("monitor.talkback_setup", &json!({"input": "In 1", "level_db": -6})).unwrap();
        assert!(e.execute("monitor.talkback_setup", &json!({"input": "Nowhere"})).is_err());
        e.execute("monitor.talkback", &json!({"on": true})).unwrap();
        let r = e.session().control_room.clone().unwrap();
        assert_eq!((r.active_set, r.sets[1].output.as_str(), r.sets[1].level_db, r.dim, r.mono), (1, "Small Speakers", -10.0, true, true));
        assert_eq!(r.monitor_db(), -10.0 - 2.0 - 20.0);
        // Saved with the session; talkback is never saved on.
        let back = soundcraft_model::Session::from_json(&e.session().to_json().unwrap()).unwrap();
        let rb = back.control_room.unwrap();
        assert_eq!((rb.sets.len(), rb.cues.len(), rb.talkback.input.as_str(), rb.talkback.on), (2, 1, "In 1", false));
        // Removing the cue takes its sends.
        assert_eq!(e.execute("monitor.cue_remove", &json!({"cue": 0})).unwrap()["removed_sends"], 1);
        assert!(e.session().track(t).unwrap().mixer.sends[0].is_none());
        e.execute("monitor.control_room", &json!({"on": false})).unwrap();
        assert!(e.session().control_room.is_none());
    }
}
