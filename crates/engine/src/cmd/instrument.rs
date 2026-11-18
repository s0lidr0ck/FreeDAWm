//! An instrument track's instrument (`mix.instrument`): the plugin that turns the track's MIDI
//! into audio, built in or hosted (CLAP, VST3, Audio Units). The mixer plays `Track::instrument`;
//! the instrument is otherwise only set when the track is created (`track.new`).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{Insert, TrackId, TrackKind};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.instrument",
            "Set Instrument",
            [],
            None,
            "{track?, plugin: id, params?: {id: value}, preset?: name} — an instrument track's instrument: built-in, `clap:`, `vst3:` or `au:` id; `params` sets built-in parameters (clamped into range; unknown ids and other fields are errors); `preset` names the preset they came from (shown on a synth's preset display)",
            has_selection,
            set_instrument
        ),
        cmd!(
            "mix.instrument_param",
            "Set Instrument Parameter",
            [],
            None,
            "{track?, param, value}: one parameter of an instrument track's built-in instrument (clamped into range)",
            has_selection,
            set_instrument_param
        ),
        cmd!(
            "mix.instrument_output",
            "Instrument Output",
            [],
            None,
            "{track?, output: index (0: the instrument's first extra output), bus: id | null} — plays one of the instrument's extra outputs (Juke's Out 2-8) into a bus instead of its main output, or (null) back on its main output → {outputs: [{output, name, bus}]}",
            has_selection,
            set_output
        ),
        cmd!(
            "mix.new_output_track",
            "New Track for Output",
            [],
            None,
            "{track?, output: index} — a stereo bus and an aux track reading it, named after the track and the output, and the instrument's extra output played into it → {aux, bus}",
            has_selection,
            new_output_track
        ),
    ]
}

/// The names of the extra outputs `ins` has: a built-in instrument's (Juke's Out 2-8), or the
/// auxiliary output ports a hosted CLAP or VST3 instrument declared when the mix made it. Empty
/// for one without any.
pub fn output_names(ins: &Insert) -> Vec<String> {
    match soundcraft_dsp::create(&ins.plugin) {
        Some(p) => p.extra_outputs(),
        None => soundcraft_mix::inst_outs::known_outputs(&ins.plugin),
    }
}

/// The instrument track and output a command names, checked.
fn output_of(e: &Engine, id: &str, p: &Value) -> Result<(TrackId, u8, String)> {
    let t = tracks_required(e, id, p)?.first().copied().ok_or_else(|| bad(id, "no track"))?;
    let ins = e.session().track(t).and_then(|tr| tr.instrument.as_ref()).ok_or_else(|| bad(id, "the track has no instrument"))?;
    let names = output_names(ins);
    if names.is_empty() {
        return Err(bad(id, format!("`{}` has no outputs besides its main one", ins.plugin)));
    }
    let output = p.get("output").and_then(Value::as_u64).ok_or_else(|| bad(id, "`output`: which extra output (0 first)"))?;
    let name = usize::try_from(output)
        .ok()
        .and_then(|i| names.get(i).cloned())
        .ok_or_else(|| bad(id, format!("`{}` has {} extra outputs; no index {output}", ins.plugin, names.len())))?;
    Ok((t, u8::try_from(output).unwrap_or(0), name))
}

fn outputs_of(e: &Engine, t: TrackId) -> Value {
    let Some(tr) = e.session().track(t) else { return json!([]) };
    let names = tr.instrument.as_ref().map(output_names).unwrap_or_default();
    json!(tr.instrument_outs.iter().map(|o| json!({"output": o.output, "name": names.get(usize::from(o.output)), "bus": o.bus})).collect::<Vec<_>>())
}

/// Plays `output` of `t`'s instrument into `bus` (`None`: back on its main output).
fn route_output(e: &mut Engine, t: TrackId, output: u8, bus: Option<soundcraft_model::BusId>) {
    if let Some(tr) = e.session_mut().track_mut(t) {
        tr.instrument_outs.retain(|o| o.output != output);
        if let Some(bus) = bus {
            tr.instrument_outs.push(soundcraft_model::InstrumentOut { output, bus });
            tr.instrument_outs.sort_by_key(|o| o.output);
        }
    }
}

fn set_output(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.instrument_output";
    let (t, output, _) = output_of(e, C, p)?;
    let bus = match p.get("bus") {
        None => return Err(bad(C, "`bus`: a bus id, or null for the main output")),
        Some(Value::Null) => None,
        Some(v) => {
            let id = v.as_u64().map(soundcraft_model::BusId).ok_or_else(|| bad(C, "`bus` is a bus id or null"))?;
            if !e.session().busses.iter().any(|b| b.id == id) {
                return Err(bad(C, format!("no bus {}", id.0)));
            }
            Some(id)
        }
    };
    route_output(e, t, output, bus);
    Ok(json!({"outputs": outputs_of(e, t)}))
}

fn new_output_track(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.new_output_track";
    let (t, output, name) = output_of(e, C, p)?;
    let s = e.session_mut();
    let mut label = format!("{} {name}", s.track(t).map(|tr| tr.name.as_str()).unwrap_or(""));
    soundcraft_model::truncate_chars(&mut label, soundcraft_model::MAX_NAME_CHARS);
    // A bus of its own (a name already taken gets a number).
    let mut bus_name = label.clone();
    let mut n = 2;
    while s.bus_by_name(&bus_name).is_some() {
        bus_name = format!("{label} {n}");
        n += 1;
    }
    let bus = s.add_bus(&bus_name, soundcraft_model::ChannelFormat::Stereo);
    let aux = s.add_track(TrackKind::Aux, soundcraft_model::ChannelFormat::Stereo, Some(&bus_name));
    if let Some(a) = s.track_mut(aux) {
        a.mixer.input = soundcraft_model::Route::Bus(bus);
        a.mixer.output = soundcraft_model::Route::Main;
    }
    route_output(e, t, output, Some(bus));
    Ok(json!({"aux": aux, "bus": bus, "outputs": outputs_of(e, t)}))
}

/// Fields `mix.instrument` reads. Anything else (a parameter written beside `plugin`, say) used to
/// be accepted and ignored, so a synth's settings looked set while the render never changed.
const FIELDS: [&str; 5] = ["track", "tracks", "plugin", "params", "preset"];

fn set_instrument(e: &mut Engine, p: &Value) -> Result<Value> {
    if let Some(k) = p.as_object().and_then(|o| o.keys().find(|k| !FIELDS.contains(&k.as_str()))) {
        return Err(bad(
            "mix.instrument",
            format!("unknown field `{k}`: an instrument's parameters go in `params` ({{id: value}}), or one at a time in mix.instrument_param"),
        ));
    }
    let plugin = str_param(p, "plugin").ok_or_else(|| bad("mix.instrument", "`plugin` required"))?;
    let info = super::clap::plugin_info(plugin).ok_or_else(|| bad("mix.instrument", format!("unknown plugin `{plugin}`")))?;
    if !info.is_instrument {
        return Err(bad("mix.instrument", format!("`{plugin}` is not an instrument")));
    }
    let t = tracks_required(e, "mix.instrument", p)?.first().copied().ok_or_else(|| bad("mix.instrument", "no track"))?;
    if e.session().track(t).map(|tr| tr.kind) != Some(TrackKind::Instrument) {
        return Err(bad("mix.instrument", "only instrument tracks have an instrument"));
    }
    // Validate the parameters before touching the session.
    let mut values = Vec::new();
    if let Some(v) = p.get("params") {
        let obj = v.as_object().ok_or_else(|| bad("mix.instrument", "`params` must be an object"))?;
        for (k, v) in obj {
            let pi = info.params.iter().find(|pi| pi.id == k).ok_or_else(|| bad("mix.instrument", format!("`{plugin}` has no parameter `{k}`")))?;
            let x = v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| bad("mix.instrument", format!("`{k}` must be a number")))?;
            values.push((k.clone(), pi.clamp(x as f32)));
        }
    }
    let mut ins = Insert::new(info.id);
    for pi in info.params {
        ins.params.insert(pi.id.to_string(), pi.default);
    }
    for (k, v) in values {
        ins.params.insert(k, v);
    }
    // The preset these settings came from (a synth panel's preset display shows it).
    match p.get("preset") {
        None | Some(Value::Null) => {}
        Some(Value::String(s)) => ins.preset = s.chars().take(128).collect(),
        Some(_) => return Err(bad("mix.instrument", "`preset` must be a name")),
    }
    if let Some(tr) = e.session_mut().track_mut(t) {
        tr.instrument = Some(ins);
        // Another instrument's outputs are its own: they start on its main output.
        tr.instrument_outs.clear();
    }
    Ok(json!({"track": t, "plugin": info.id}))
}

fn set_instrument_param(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.instrument_param";
    let param = str_param(p, "param").ok_or_else(|| bad(id, "`param` required"))?.to_string();
    let value = p.get("value").and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| bad(id, "`value` must be a number"))? as f32;
    let t = tracks_required(e, id, p)?.first().copied().ok_or_else(|| bad(id, "no track"))?;
    let ins = e.session_mut().track_mut(t).and_then(|tr| tr.instrument.as_mut()).ok_or_else(|| bad(id, "the track has no instrument"))?;
    // A hosted instrument keeps its parameters in its own state too; the session's copy follows
    // what its editor reports, so a render made from the stored params plays what was heard.
    let info = super::clap::plugin_info(&ins.plugin).ok_or_else(|| bad(id, format!("`{}` is not a known instrument", ins.plugin)))?;
    let pi = info.param(&param).ok_or_else(|| bad(id, format!("`{}` has no parameter `{param}`", info.id)))?;
    let v = pi.clamp(value);
    ins.params.insert(param.clone(), v);
    // A move in the instrument's window or drawer is written as automation in Touch, Latch and
    // Write, as a chain module's is.
    e.automation_changed(t, soundcraft_model::AutoParam::Plugin { module: soundcraft_model::INSTRUMENT_MODULE, param: param.clone() });
    Ok(json!({"track": t, "param": param, "value": v}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_midi::{Note, Sequence};
    use soundcraft_model::{Clip, TrackId};
    use soundcraft_time::{Range, TICKS_PER_QUARTER};

    fn track(e: &mut Engine, kind: &str) -> TrackId {
        e.execute("track.new", &json!({"kind": kind})).unwrap();
        e.session().tracks.last().unwrap().id
    }

    fn instrument_of(e: &Engine, t: TrackId) -> Option<String> {
        e.session().track(t).and_then(|tr| tr.instrument.as_ref()).map(|i| i.plugin.clone())
    }

    /// Parity section 7: an instrument's parameter moved while playing in Touch is written to its
    /// lane, as a chain module's is.
    #[test]
    fn an_instrument_parameter_moved_in_touch_is_written() {
        let mut e = Engine::default();
        let t = track(&mut e, "instrument");
        e.execute("mix.automation_mode", &json!({"tracks": [t.0], "mode": "touch"})).unwrap();
        let q = i64::from(e.session().sample_rate.hz()) / 10;
        e.transport.playing = true;
        e.transport.position = 0;
        e.automation_pass(true);
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": "level", "value": -18.0})).unwrap();
        e.transport.position = q;
        assert_eq!(e.automation_pass(true), 1);
        let key = soundcraft_model::AutoParam::Plugin { module: soundcraft_model::INSTRUMENT_MODULE, param: "level".into() };
        let lane = e.session().track(t).unwrap().lane(&key).expect("a lane on the instrument");
        assert!(lane.points.iter().any(|p| p.value == -18.0), "{:?}", lane.points);
    }

    /// Juke's Out 3 on a track of its own: a bus, an aux reading it, the output played into it;
    /// one undo step; another instrument starts on its main output again.
    #[test]
    fn an_instrument_output_gets_a_track_of_its_own() {
        let mut e = Engine::default();
        let t = track(&mut e, "instrument");
        assert!(e.execute("mix.new_output_track", &json!({"track": t.0, "output": 0})).is_err(), "the synth has no extra outputs");
        e.execute("mix.instrument", &json!({"track": t.0, "plugin": "juke"})).unwrap();
        assert!(e.execute("mix.new_output_track", &json!({"track": t.0, "output": 7})).is_err(), "Out 2 to Out 8: seven");
        let tracks = e.session().tracks.len();
        let r = e.execute("mix.new_output_track", &json!({"track": t.0, "output": 1})).unwrap();
        assert_eq!(r["outputs"][0]["name"], "Out 3");
        let s = e.session();
        let aux = s.tracks.last().unwrap();
        assert_eq!(s.tracks.len(), tracks + 1);
        assert_eq!(aux.kind, soundcraft_model::TrackKind::Aux);
        let bus = s.track(t).unwrap().instrument_outs[0].bus;
        assert_eq!(aux.mixer.input, soundcraft_model::Route::Bus(bus));
        assert!(aux.name.ends_with("Out 3"), "{}", aux.name);
        // Back on the main output, then into the bus again.
        e.execute("mix.instrument_output", &json!({"track": t.0, "output": 1, "bus": null})).unwrap();
        assert!(e.session().track(t).unwrap().instrument_outs.is_empty());
        e.execute("mix.instrument_output", &json!({"track": t.0, "output": 1, "bus": bus.0})).unwrap();
        assert!(e.execute("mix.instrument_output", &json!({"track": t.0, "output": 1, "bus": 999_999})).is_err());
        // Kept with the session.
        let back: soundcraft_model::Session = serde_json::from_str(&serde_json::to_string(e.session()).unwrap()).unwrap();
        assert_eq!(back.track(t).unwrap().instrument_outs, e.session().track(t).unwrap().instrument_outs);
        e.execute("mix.instrument", &json!({"track": t.0, "plugin": "subtractive_synth"})).unwrap();
        assert!(e.session().track(t).unwrap().instrument_outs.is_empty());
        e.execute("edit.undo", &json!({})).unwrap();
        e.execute("edit.undo", &json!({})).unwrap();
        e.execute("edit.undo", &json!({})).unwrap();
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().tracks.len(), tracks, "the new track was one step");
    }

    #[test]
    fn sets_an_instrument_tracks_instrument_with_defaults_and_undoes() {
        let mut e = Engine::default();
        let t = track(&mut e, "instrument");
        assert_eq!(instrument_of(&e, t).as_deref(), Some("subtractive_synth"));
        let r = e.execute("mix.instrument", &json!({"track": t.0, "plugin": "drum_synth"})).unwrap();
        assert_eq!(r["plugin"], "drum_synth");
        assert_eq!(instrument_of(&e, t).as_deref(), Some("drum_synth"));
        let params = e.session().track(t).and_then(|tr| tr.instrument.as_ref()).map(|i| i.params.len());
        assert_eq!(params, soundcraft_dsp::plugin_info("drum_synth").map(|p| p.params.len()), "every parameter at its default");
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(instrument_of(&e, t).as_deref(), Some("subtractive_synth"));
    }

    #[test]
    fn sets_a_built_in_instruments_parameters() {
        let mut e = Engine::default();
        let t = track(&mut e, "instrument");
        let r = e.execute("mix.instrument_param", &json!({"track": t.0, "param": "cutoff", "value": 5000})).unwrap();
        assert_eq!(r["value"], 5000.0);
        let r = e.execute("mix.instrument_param", &json!({"track": t.0, "param": "cutoff", "value": 1.0e9})).unwrap();
        assert_eq!(r["value"], 20000.0, "clamped");
        let v = e.session().track(t).and_then(|tr| tr.instrument.as_ref()).and_then(|i| i.params.get("cutoff").copied());
        assert_eq!(v, Some(20000.0));
        assert!(e.execute("mix.instrument_param", &json!({"track": t.0, "param": "nope", "value": 1})).is_err());
        assert!(e.execute("mix.instrument_param", &json!({"track": t.0, "param": "cutoff"})).is_err());
        assert!(e.execute("mix.instrument_param", &json!({"track": t.0, "param": "cutoff", "value": "loud"})).is_err());
        let audio = track(&mut e, "audio");
        assert!(e.execute("mix.instrument_param", &json!({"track": audio.0, "param": "cutoff", "value": 1})).is_err());
        e.execute("edit.undo", &json!({})).unwrap();
    }

    /// One note of 440 Hz (A4) on the track, `secs` of render, and the RMS of the result.
    fn rms_of_a_note(e: &mut Engine, t: TrackId, secs: i64) -> f32 {
        let q = TICKS_PER_QUARTER;
        let mut seq = Sequence::default();
        seq.notes.push(Note { probability: 100, pitch: 93, velocity: 100, muted: false, release_velocity: 64, channel: 0, start: 0, length: 2 * q });
        let s = e.session_mut();
        let cid = s.new_clip_id();
        crate::edit::place_clip(s, t, Clip::midi(cid, "note", 0, 48_000 * 4, seq));
        let out = soundcraft_mix::render_range(e.session(), Range::new(0, 48_000 * secs), 512);
        let n = out[0].len().max(1) as f32;
        (out[0].iter().map(|x| x * x).sum::<f32>() / n).sqrt()
    }

    /// A synth panel's preset display: `preset` names the preset the settings came from, and is
    /// a string or nothing.
    #[test]
    fn the_instrument_keeps_the_name_of_its_preset() {
        let mut e = Engine::default();
        let t = track(&mut e, "instrument");
        e.execute("mix.instrument", &json!({"track": t.0, "plugin": "subtractive_synth", "params": {"cutoff": 420}, "preset": "Round Bass"}))
            .unwrap();
        let ins = e.session().track(t).and_then(|tr| tr.instrument.clone()).unwrap();
        assert_eq!((ins.preset.as_str(), ins.params.get("cutoff").copied()), ("Round Bass", Some(420.0)));
        assert!(e.execute("mix.instrument", &json!({"track": t.0, "plugin": "subtractive_synth", "preset": 3})).is_err());
        e.execute("mix.instrument", &json!({"track": t.0, "plugin": "subtractive_synth"})).unwrap();
        assert_eq!(e.session().track(t).and_then(|tr| tr.instrument.clone()).unwrap().preset, "<factory default>");
    }

    #[test]
    fn parameters_given_with_the_instrument_reach_the_render_and_unknown_fields_are_refused() {
        // A high note through a closed and an open filter: the cutoff has to change the render.
        let mut closed = Engine::default();
        let t = track(&mut closed, "instrument");
        let r = closed.execute("mix.instrument", &json!({"track": t.0, "plugin": "subtractive_synth", "params": {"cutoff": 200}})).unwrap();
        assert_eq!(r["plugin"], "subtractive_synth");
        let stored = closed.session().track(t).and_then(|tr| tr.instrument.as_ref()).and_then(|i| i.params.get("cutoff").copied());
        assert_eq!(stored, Some(200.0));
        let mut open = Engine::default();
        let o = track(&mut open, "instrument");
        open.execute("mix.instrument", &json!({"track": o.0, "plugin": "subtractive_synth", "params": {"cutoff": 12_000}})).unwrap();
        let (quiet, loud) = (rms_of_a_note(&mut closed, t, 2), rms_of_a_note(&mut open, o, 2));
        assert!(loud > quiet * 3.0, "cutoff 12000 plays a high note much louder than 200: {loud} vs {quiet}");
        // Values are clamped into range; the instrument starts from its defaults for the rest.
        let mut e = Engine::default();
        let t = track(&mut e, "instrument");
        e.execute("mix.instrument", &json!({"track": t.0, "plugin": "subtractive_synth", "params": {"cutoff": 1.0e9}})).unwrap();
        let ins = e.session().track(t).and_then(|tr| tr.instrument.clone()).unwrap();
        assert_eq!(ins.params.get("cutoff"), Some(&20_000.0));
        assert_eq!(ins.params.len(), soundcraft_dsp::plugin_info("subtractive_synth").map(|p| p.params.len()).unwrap());

        // Nothing is accepted and then ignored: a parameter beside `plugin`, an unknown id, a value
        // that is not a number and a `params` that is not an object are errors, and change nothing.
        let mut e = Engine::default();
        let t = track(&mut e, "instrument");
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": "cutoff", "value": 777})).unwrap();
        let before = e.session().track(t).and_then(|tr| tr.instrument.clone());
        let depth = e.revision;
        for bad in [
            json!({"track": t.0, "plugin": "drum_synth", "cutoff": 200}),
            json!({"track": t.0, "plugin": "drum_synth", "params": {"nope": 1}}),
            json!({"track": t.0, "plugin": "drum_synth", "params": {"decay": "long"}}),
            json!({"track": t.0, "plugin": "drum_synth", "params": 3}),
        ] {
            let err = e.execute("mix.instrument", &bad).unwrap_err().to_string();
            assert!(!err.is_empty(), "{bad}");
            assert_eq!(e.session().track(t).and_then(|tr| tr.instrument.clone()), before, "{bad}: the instrument is untouched");
            assert_eq!(e.revision, depth, "{bad}: nothing was recorded");
        }
        let err = e.execute("mix.instrument", &json!({"track": t.0, "plugin": "drum_synth", "cutoff": 200})).unwrap_err().to_string();
        assert!(err.contains("cutoff") && err.contains("params"), "{err}");
    }

    #[test]
    fn refuses_effects_unknown_plugins_and_other_track_kinds() {
        let mut e = Engine::default();
        let inst = track(&mut e, "instrument");
        let audio = track(&mut e, "audio");
        assert!(e.execute("mix.instrument", &json!({"track": inst.0, "plugin": "eq_7band"})).is_err(), "an effect");
        assert!(e.execute("mix.instrument", &json!({"track": inst.0})).is_err(), "no plugin");
        assert!(e.execute("mix.instrument", &json!({"track": audio.0, "plugin": "drum_synth"})).is_err(), "an audio track");
        assert_eq!(instrument_of(&e, inst).as_deref(), Some("subtractive_synth"));
        assert_eq!(instrument_of(&e, audio), None);
    }

    // An unknown `vst3:` id is looked up in the scan of the system VST3 folders.
    #[test]
    #[ignore = "loads every installed VST3 plugin (slow; some abort the test process); run with --ignored"]
    fn refuses_unknown_vst3_instruments() {
        let mut e = Engine::default();
        let inst = track(&mut e, "instrument");
        assert!(e.execute("mix.instrument", &json!({"track": inst.0, "plugin": "vst3:00000000000000000000000000000000"})).is_err());
        assert_eq!(instrument_of(&e, inst).as_deref(), Some("subtractive_synth"));
    }
}
