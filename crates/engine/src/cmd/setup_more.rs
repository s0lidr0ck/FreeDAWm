//! More Options toggles, the Setup menu (hardware, playback engine, I/O, timecode, click,
//! shortcuts, MIDI setup) and File › Import › Session Data.

use super::more_util::{set_text, store_value, text, toggle_flag, values_with_prefix};
use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{BusId, ChannelFormat, ClipContent, InputPath, OutputPath, Route, Session, SourceId, TrackId, TrackKind};
use soundcraft_time::Timecode;
use std::path::Path;

macro_rules! option {
    ($id:literal, $label:literal, $flag:literal) => {
        cmd!(noundo $id, $label, ["Options"], None, "{value?: bool} (toggles when omitted)", always, |e, p| toggle_flag(e, p, $flag))
    };
}

const MIDI_FILTER_KINDS: [&str; 8] = ["notes", "pitch_bend", "mono_aftertouch", "poly_aftertouch", "program", "cc", "sysex", "realtime"];

pub fn specs() -> Vec<CommandSpec> {
    vec![
        option!("options.destructive_punch", "DestructivePunch", "options.destructive_punch"),
        option!("options.transport_online", "Transport Online", "options.transport_online"),
        option!("options.dynamic_transport", "Dynamic Transport", "options.dynamic_transport"),
        option!(
            "options.marker_target_follows_selection",
            "Marker Target Follows First Selected Track",
            "options.marker_target_follows_first_selected_track"
        ),
        option!("options.midi_thru", "MIDI Thru", "options.midi_thru"),
        option!("options.auto_spot_clips", "Auto-Spot Clips", "options.auto_spot_clips"),
        option!("options.keyboard_lock", "Edit/Tool Mode Keyboard Lock", "options.edit_tool_mode_keyboard_lock"),
        option!("options.calibration_mode", "Calibration Mode", "options.calibration_mode"),
        option!("options.midi_live_mode", "MIDI Live Mode", "options.midi_live_mode"),
        option!("options.low_latency_monitoring", "Low Latency Monitoring", "options.low_latency_monitoring"),
        cmd!(
            "options.prepare_dpe_tracks",
            "Prepare DPE Tracks",
            ["Options"],
            None,
            "{tracks?} — consolidates each record-armed (or given) audio track into one continuous clip for destructive punch",
            always,
            prepare_dpe
        ),
        cmd!(noundo "setup.hardware", "Hardware...", ["Setup"], None, "{device?, buffer_size?: 16..8192} — audio interface settings (sample rate is the session's)", always, |e, p| engine_settings(e, p)),
        cmd!(noundo "setup.playback_engine", "Playback Engine...", ["Setup"], None, "{device?, buffer_size?: 16..8192, delay_compensation?: bool}", always, |e, p| {
            if let Some(v) = p.get("delay_compensation").and_then(Value::as_bool) {
                e.session_mut().edit.delay_compensation = v;
            }
            engine_settings(e, p)
        }),
        cmd!(noundo "setup.disk_allocation", "Disk Allocation...", ["Setup"], None, "{path?, track?} — the record folder for the session, or for one track", always, disk_allocation),
        cmd!(noundo "setup.peripherals", "Peripherals...", ["Setup"], None, "{settings?: {name: number|bool|string}} — control surface / sync device settings", always, peripherals),
        cmd!(
            "setup.io",
            "I/O...",
            ["Setup"],
            None,
            "{action?: list|create_bus|rename_bus|delete_bus|create_output|rename_output|delete_output|set_output_channel|set_output_format|create_input|rename_input|delete_input|set_input_channel|set_input_format|auto_inputs, name?, new_name?, format?: busses and outputs any format, default stereo (set_output_format: any format, not the main's: use setup.main_format); inputs mono|stereo, default mono, first_channel?: 0..1023, channels?: 1..64 (auto_inputs)}",
            always,
            io_setup
        ),
        cmd!(
            "setup.current_timecode",
            "Current Timecode Position...",
            ["Setup"],
            None,
            "{timecode: 'HH:MM:SS:FF'} — sets the session start so the insertion point reads this timecode",
            always,
            current_timecode
        ),
        cmd!(noundo "setup.current_feet_frames", "Current Feet+Frames Position...", ["Setup"], None, "{feet_frames: 'F+FF'} — the feet+frames reading at the insertion point", always, current_feet_frames),
        cmd!(noundo "setup.external_timecode_offset", "External Timecode Offset...", ["Setup"], None, "{frames?: ±, samples?: ±} — offset applied to incoming timecode", always, |e, p| offset_setting(e, p, "sync.external_offset")),
        cmd!(noundo "setup.video_sync_offset", "Video Sync Offset...", ["Setup"], None, "{frames?: ±, samples?: ±} — offset between audio and video playback", always, |e, p| offset_setting(e, p, "video.sync_offset")),
        cmd!(noundo "setup.click_countoff", "Click/Countoff...", ["Setup"], None, "{volume_db?: -60..12, accent_note?: 0..127, normal_note?: 0..127, accent_velocity?, normal_velocity?, countoff?: bool, countoff_bars?: 1..16, only_during_record?: bool}", always, click_countoff),
        cmd!(query "setup.keyboard_shortcuts", "Keyboard Shortcuts...", ["Setup"], None, "{filter?} — every command shortcut", always, |_, p| {
            let f = str_param(p, "filter").map(str::to_ascii_lowercase);
            let rows: Vec<Value> = command_specs()
                .iter()
                .filter_map(|c| c.shortcut.map(|sc| (c, sc)))
                .filter(|(c, _)| f.as_ref().is_none_or(|f| c.id.contains(f.as_str()) || c.label.to_ascii_lowercase().contains(f.as_str())))
                .map(|(c, sc)| json!({"id": c.id, "label": c.label, "menu": c.menu.join(" > "), "shortcut": sc}))
                .collect();
            Ok(json!({"shortcuts": rows}))
        }),
        cmd!(noundo "setup.midi_studio", "MIDI Studio...", ["Setup", "MIDI"], None, "{devices?: [names]} — the MIDI devices in the studio", always, |e, p| {
            let s = e.session_mut();
            if let Some(a) = p.get("devices").and_then(Value::as_array) {
                let names: Vec<&str> = a.iter().filter_map(Value::as_str).take(64).collect();
                set_text(&mut s.edit, "midi.studio.devices", Some(&names.join("|")));
            }
            let devices: Vec<String> = text(&s.edit, "midi.studio.devices").map(|t| t.split('|').map(str::to_string).collect()).unwrap_or_default();
            Ok(json!({"devices": devices}))
        }),
        cmd!(noundo "setup.midi_beat_clock", "MIDI Beat Clock...", ["Setup", "MIDI"], None, "{enabled?: bool, offset?: samples, destination?: name}", always, |e, p| {
            let s = e.session_mut();
            if let Some(v) = p.get("enabled").and_then(Value::as_bool) {
                s.edit.set_flag("midi.beat_clock", v);
            }
            store_value(&mut s.edit, p, "offset", "midi.beat_clock.offset", -1e7, 1e7);
            if let Some(d) = str_param(p, "destination") {
                set_text(&mut s.edit, "midi.beat_clock.destination", Some(d));
            }
            Ok(json!({"enabled": s.edit.flag("midi.beat_clock"), "offset": s.edit.value("midi.beat_clock.offset", 0.0), "destination": text(&s.edit, "midi.beat_clock.destination")}))
        }),
        cmd!(noundo "setup.midi_input_filter", "MIDI Input Filter...", ["Setup", "MIDI"], None, "{filter?: [notes|pitch_bend|mono_aftertouch|poly_aftertouch|program|cc|sysex|realtime], allow?: [...]} — filtered kinds are not recorded", always, midi_filter),
        cmd!(noundo "setup.midi_input_devices", "MIDI Input Devices...", ["Setup", "MIDI"], None, "{enable?: [names], disable?: [names]}", always, midi_inputs),
        cmd!(noundo "setup.transcription_settings", "Transcription Settings...", ["Setup"], None, "{enabled?: bool, language?: 'en'}", always, |e, p| {
            let s = e.session_mut();
            if let Some(v) = p.get("enabled").and_then(Value::as_bool) {
                s.edit.set_flag("transcription.enabled", v);
            }
            if let Some(l) = str_param(p, "language") {
                set_text(&mut s.edit, "transcription.language", Some(l));
            }
            Ok(json!({"enabled": s.edit.flag("transcription.enabled"), "language": text(&s.edit, "transcription.language").unwrap_or_else(|| "en".into())}))
        }),
        cmd!(
            "file.import_session_data",
            "Session Data...",
            ["File", "Import"],
            Some("Alt+Shift+I"),
            "{path, tracks?: [names]} — imports tracks (clips, playlists, mixer, automation, audio) from another session",
            always,
            import_session_data
        ),
    ]
}

fn prepare_dpe(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "options.prepare_dpe_tracks";
    let s = e.session();
    let tracks: Vec<TrackId> = if p.get("tracks").is_some() || p.get("track").is_some() {
        tracks_param(e, id, p)?
    } else {
        s.tracks.iter().filter(|t| t.mixer.record_arm).map(|t| t.id).collect()
    };
    let tracks: Vec<TrackId> = tracks.into_iter().filter(|t| s.track(*t).is_some_and(|x| x.kind == TrackKind::Audio)).collect();
    if tracks.is_empty() {
        return Err(bad(id, "record-enable (or pass) the audio tracks to prepare"));
    }
    let end = s.content_end();
    if end <= 0 {
        return Err(bad(id, "there is no audio to prepare"));
    }
    let r = soundcraft_time::Range::new(0, end);
    let mut n = 0;
    for t in tracks {
        // Each track keeps its own length (its material plus a second), as before.
        let tr = crate::io::bounded_to_track(e.session(), t, r);
        if crate::io::consolidate(e, id, t, tr)? {
            n += 1;
        }
    }
    e.session_mut().edit.set_flag("options.dpe_prepared", true);
    Ok(json!({"prepared": n}))
}

fn engine_settings(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session_mut();
    if let Some(b) = p.get("buffer_size").and_then(Value::as_u64) {
        let b = b.clamp(16, 8192).next_power_of_two().min(8192);
        s.edit.values.insert("engine.buffer_size".into(), b as f64);
    }
    if let Some(d) = str_param(p, "device") {
        set_text(&mut s.edit, "engine.device", Some(d));
    }
    Ok(json!({
        "device": text(&s.edit, "engine.device"),
        "buffer_size": s.edit.value("engine.buffer_size", 1024.0),
        "sample_rate": s.sample_rate.hz(),
        "bit_depth": s.bit_depth,
        "delay_compensation": s.edit.delay_compensation,
    }))
}

fn disk_allocation(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "setup.disk_allocation";
    let t = track_param(e, id, p, "track")?;
    let s = e.session_mut();
    if let Some(path) = str_param(p, "path") {
        let key = t.map_or_else(|| "disk.root".to_string(), |t| format!("disk.track.{}", t.0));
        set_text(&mut s.edit, &key, Some(path));
    }
    let root = text(&s.edit, "disk.root").unwrap_or_else(|| "Audio Files".into());
    let rows: Vec<Value> = s
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio)
        .map(|t| json!({"track": t.id, "name": t.name, "path": text(&s.edit, &format!("disk.track.{}", t.id.0)).unwrap_or_else(|| root.clone())}))
        .collect();
    Ok(json!({"root": root, "tracks": rows}))
}

fn peripherals(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session_mut();
    if let Some(m) = p.get("settings").and_then(Value::as_object) {
        for (k, v) in m.iter().take(256) {
            let key: String = k.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')).take(64).collect();
            if key.is_empty() {
                continue;
            }
            let key = format!("peripherals.{key}");
            match v {
                Value::Bool(b) => s.edit.set_flag(&key, *b),
                Value::Number(n) => {
                    if let Some(x) = n.as_f64().filter(|x| x.is_finite()) {
                        s.edit.values.insert(key, x);
                    }
                }
                Value::String(t) => set_text(&mut s.edit, &key, Some(t)),
                _ => {}
            }
        }
    }
    let flags: Vec<&String> = s.edit.flags.iter().filter(|f| f.starts_with("peripherals.")).collect();
    Ok(json!({"values": values_with_prefix(&s.edit, "peripherals."), "flags": flags}))
}

fn io_state(s: &Session) -> Value {
    json!({"busses": s.busses, "inputs": s.inputs, "outputs": s.outputs})
}

// Case-insensitive name match (Unicode), as the model's resolver matches inputs and outputs.
use soundcraft_model::same_name as same;

/// Refuses a path or bus name that a route always reads as something else ("Main", "None", "Out 1-2"…,
/// see `Route::reserved`): the path could never be picked. The main output may keep a main name.
fn check_path_name(id: &str, name: &str, main: bool) -> Result<()> {
    // A Band musician's name ("Musician 2", "musician:2") always routes to the musician.
    if super::track::musician_route(name).is_some() {
        return Err(bad(id, format!("`{}` is a reserved route name (a Band musician)", name.trim())));
    }
    match Route::reserved(name) {
        Some(Route::Main) if main => Ok(()),
        Some(_) => Err(bad(id, format!("`{}` is a reserved route name", name.trim()))),
        None => Ok(()),
    }
}

/// `first_channel` (0-based), clamped to 0..=1023.
fn first_channel(p: &Value) -> u16 {
    i64_or(p, "first_channel", 0).clamp(0, 1023) as u16
}

/// An input's format: mono or stereo.
fn input_format(id: &str, f: Option<&str>) -> Result<ChannelFormat> {
    match f.and_then(ChannelFormat::from_id) {
        Some(f @ (ChannelFormat::Mono | ChannelFormat::Stereo)) => Ok(f),
        _ => Err(bad(id, "inputs are mono or stereo")),
    }
}

fn io_setup(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "setup.io";
    let action = str_param(p, "action").unwrap_or("list");
    if action == "list" {
        return Ok(io_state(e.session()));
    }
    if action == "auto_inputs" {
        let n = p.get("channels").and_then(Value::as_i64).ok_or_else(|| bad(id, "`channels` (1..64) required"))?.clamp(1, 64);
        let s = e.session_mut();
        for k in 1..=n {
            let name = format!("In {k}");
            if !s.inputs.iter().any(|i| same(&i.name, &name)) {
                s.inputs.push(InputPath { name, first_channel: (k - 1) as u16, format: ChannelFormat::Mono });
            }
        }
        return Ok(io_state(s));
    }
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(id, "`name` required"))?;
    let name: String = name.chars().take(soundcraft_model::MAX_NAME_CHARS).collect();
    let new_name = str_param(p, "new_name")
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(|n| n.chars().take(soundcraft_model::MAX_NAME_CHARS).collect::<String>());
    // Busses and outputs: any format, stereo by default. Inputs check theirs below.
    let format_or_stereo = match str_param(p, "format") {
        Some(f) if !action.ends_with("_input") => ChannelFormat::from_id(f).ok_or_else(|| bad(id, format!("unknown format `{f}`")))?,
        _ => ChannelFormat::Stereo,
    };
    let has_channel = p.get("first_channel").is_some_and(Value::is_number);
    let s = e.session_mut();
    match action {
        "create_input" => {
            check_path_name(id, &name, false)?;
            if s.inputs.iter().any(|i| same(&i.name, &name)) {
                return Err(bad(id, format!("an input named `{name}` exists")));
            }
            let format = match str_param(p, "format") {
                None => ChannelFormat::Mono,
                f => input_format(id, f)?,
            };
            s.inputs.push(InputPath { name, first_channel: first_channel(p), format });
        }
        "rename_input" => {
            let to = new_name.ok_or_else(|| bad(id, "`new_name` required"))?;
            check_path_name(id, &to, false)?;
            if s.inputs.iter().any(|i| same(&i.name, &to) && !same(&i.name, &name)) {
                return Err(bad(id, format!("an input named `{to}` exists")));
            }
            let i = s.inputs.iter_mut().find(|i| same(&i.name, &name)).ok_or_else(|| bad(id, format!("no input `{name}`")))?;
            i.name = to.clone();
            for t in &mut s.tracks {
                if matches!(&t.mixer.input, Route::Hardware(h) if same(h, &name)) {
                    t.mixer.input = Route::Hardware(to.clone());
                }
            }
        }
        "delete_input" => {
            let before = s.inputs.len();
            s.inputs.retain(|i| !same(&i.name, &name));
            if s.inputs.len() == before {
                return Err(bad(id, format!("no input `{name}`")));
            }
            for t in &mut s.tracks {
                if matches!(&t.mixer.input, Route::Hardware(h) if same(h, &name)) {
                    t.mixer.input = Route::None;
                }
            }
        }
        "set_input_channel" => {
            if !has_channel {
                return Err(bad(id, "`first_channel` required"));
            }
            let i = s.inputs.iter_mut().find(|i| same(&i.name, &name)).ok_or_else(|| bad(id, format!("no input `{name}`")))?;
            i.first_channel = first_channel(p);
        }
        "set_input_format" => {
            let format = input_format(id, str_param(p, "format"))?;
            let i = s.inputs.iter_mut().find(|i| same(&i.name, &name)).ok_or_else(|| bad(id, format!("no input `{name}`")))?;
            i.format = format;
        }
        "set_output_format" => {
            if str_param(p, "format").is_none() {
                return Err(bad(id, "`format` required"));
            }
            // The main's format is the session's main format (setup.main_format).
            let index = s.output_index(&name).ok_or_else(|| bad(id, format!("no output `{name}`")))?;
            if index == 0 {
                return Err(bad(id, "the main output's format is set with setup.main_format"));
            }
            if let Some(o) = s.outputs.get_mut(index) {
                o.format = format_or_stereo;
            }
        }
        "set_output_channel" => {
            if !has_channel {
                return Err(bad(id, "`first_channel` required"));
            }
            let o = s.outputs.iter_mut().find(|o| same(&o.name, &name)).ok_or_else(|| bad(id, format!("no output `{name}`")))?;
            o.first_channel = first_channel(p);
        }
        "create_bus" => {
            check_path_name(id, &name, false)?;
            if s.bus_by_name(&name).is_some() {
                return Err(bad(id, format!("a bus named `{name}` exists")));
            }
            s.add_bus(&name, format_or_stereo);
        }
        "rename_bus" => {
            let to = new_name.ok_or_else(|| bad(id, "`new_name` required"))?;
            check_path_name(id, &to, false)?;
            if s.bus_by_name(&to).is_some() {
                return Err(bad(id, format!("a bus named `{to}` exists")));
            }
            let b = s.busses.iter_mut().find(|b| b.name.eq_ignore_ascii_case(&name)).ok_or_else(|| bad(id, format!("no bus `{name}`")))?;
            b.name = to;
        }
        "delete_bus" => {
            let bid: BusId = s.bus_by_name(&name).map(|b| b.id).ok_or_else(|| bad(id, format!("no bus `{name}`")))?;
            s.busses.retain(|b| b.id != bid);
            for t in &mut s.tracks {
                if t.mixer.input == Route::Bus(bid) {
                    t.mixer.input = Route::None;
                }
                if t.mixer.output == Route::Bus(bid) {
                    t.mixer.output = Route::None;
                }
                for slot in &mut t.mixer.sends {
                    if slot.as_ref().is_some_and(|x| x.target == Route::Bus(bid)) {
                        *slot = None;
                    }
                }
            }
        }
        "create_output" => {
            check_path_name(id, &name, false)?;
            if s.outputs.iter().any(|o| same(&o.name, &name)) {
                return Err(bad(id, format!("an output named `{name}` exists")));
            }
            s.outputs.push(OutputPath { name, first_channel: first_channel(p), format: format_or_stereo });
        }
        "rename_output" => {
            let to = new_name.ok_or_else(|| bad(id, "`new_name` required"))?;
            let index = s.output_index(&name).ok_or_else(|| bad(id, format!("no output `{name}`")))?;
            check_path_name(id, &to, index == 0)?;
            if s.outputs.iter().enumerate().any(|(i, o)| i != index && same(&o.name, &to)) {
                return Err(bad(id, format!("an output named `{to}` exists")));
            }
            let Some(o) = s.outputs.get_mut(index) else { return Err(bad(id, format!("no output `{name}`"))) };
            let old = std::mem::replace(&mut o.name, to.clone());
            for t in &mut s.tracks {
                if matches!(&t.mixer.output, Route::Hardware(h) if same(h, &old)) {
                    t.mixer.output = Route::Hardware(to.clone());
                }
            }
        }
        "delete_output" => {
            if s.output_index(&name) == Some(0) {
                return Err(bad(id, "the main output can't be deleted"));
            }
            let before = s.outputs.len();
            s.outputs.retain(|o| !same(&o.name, &name));
            if s.outputs.len() == before {
                return Err(bad(id, format!("no output `{name}`")));
            }
        }
        other => return Err(bad(id, format!("unknown action `{other}`"))),
    }
    Ok(io_state(s))
}

fn current_timecode(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "setup.current_timecode";
    let tc = str_param(p, "timecode").ok_or_else(|| bad(id, "`timecode` required (HH:MM:SS:FF)"))?;
    let tc = Timecode::parse(tc).map_err(|err| bad(id, err.to_string()))?;
    let s = e.session_mut();
    let at = s.edit.selection.start.max(0);
    let abs = tc.to_samples(s.frame_rate, s.sample_rate);
    s.timecode_start = abs - at;
    Ok(json!({"timecode_start": s.timecode_start, "at": at}))
}

fn current_feet_frames(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "setup.current_feet_frames";
    let text_v = str_param(p, "feet_frames").ok_or_else(|| bad(id, "`feet_frames` required (F+FF)"))?;
    let (ft, fr) = text_v.split_once('+').ok_or_else(|| bad(id, "use F+FF, e.g. 12+08"))?;
    let ft: i64 = ft.trim().parse().map_err(|_| bad(id, "bad feet"))?;
    let fr: i64 = fr.trim().parse().map_err(|_| bad(id, "bad frames"))?;
    if !(0..=10_000_000).contains(&ft) || !(0..16).contains(&fr) {
        return Err(bad(id, "feet must be 0.. and frames 0..15"));
    }
    let s = e.session_mut();
    let at = s.edit.selection.start.max(0);
    let frames_at = s.frame_rate.frames_at(at, s.sample_rate);
    let start = ft * 16 + fr - frames_at;
    s.edit.values.insert(soundcraft_model::FEET_FRAMES_START.into(), start as f64);
    Ok(json!({"feet_frames_start": start}))
}

fn offset_setting(e: &mut Engine, p: &Value, key: &str) -> Result<Value> {
    let s = e.session_mut();
    let sr = s.sample_rate;
    let per_frame = s.frame_rate.samples_per_frame(sr);
    if let Some(f) = p.get("frames").and_then(Value::as_f64).filter(|v| v.is_finite()) {
        s.edit.values.insert(key.to_string(), (f.clamp(-10_000.0, 10_000.0) * per_frame).round());
    } else if let Some(n) = p.get("samples").and_then(Value::as_f64).filter(|v| v.is_finite()) {
        let lim = sr.as_f64() * 3600.0;
        s.edit.values.insert(key.to_string(), n.clamp(-lim, lim).round());
    }
    let samples = s.edit.value(key, 0.0);
    Ok(json!({"samples": samples, "frames": samples / per_frame.max(1e-9)}))
}

fn click_countoff(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session_mut();
    store_value(&mut s.edit, p, "volume_db", "click.volume_db", -60.0, 12.0);
    store_value(&mut s.edit, p, "accent_note", "click.accent_note", 0.0, 127.0);
    store_value(&mut s.edit, p, "normal_note", "click.normal_note", 0.0, 127.0);
    store_value(&mut s.edit, p, "accent_velocity", "click.accent_velocity", 1.0, 127.0);
    store_value(&mut s.edit, p, "normal_velocity", "click.normal_velocity", 1.0, 127.0);
    if let Some(v) = p.get("countoff").and_then(Value::as_bool) {
        s.edit.countoff = v;
    }
    if let Some(b) = p.get("countoff_bars").and_then(Value::as_u64) {
        s.edit.countoff_bars = b.clamp(1, 16) as u32;
    }
    if let Some(v) = p.get("only_during_record").and_then(Value::as_bool) {
        s.edit.set_flag("click.only_during_record", v);
    }
    Ok(json!({
        "volume_db": s.edit.value("click.volume_db", -6.0),
        "accent_note": s.edit.value("click.accent_note", 84.0),
        "normal_note": s.edit.value("click.normal_note", 72.0),
        "accent_velocity": s.edit.value("click.accent_velocity", 127.0),
        "normal_velocity": s.edit.value("click.normal_velocity", 100.0),
        "countoff": s.edit.countoff,
        "countoff_bars": s.edit.countoff_bars,
        "only_during_record": s.edit.flag("click.only_during_record"),
    }))
}

fn kinds(p: &Value, key: &str) -> Vec<&'static str> {
    p.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).filter_map(|k| MIDI_FILTER_KINDS.iter().copied().find(|x| *x == k)).collect())
        .unwrap_or_default()
}

fn midi_filter(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session_mut();
    for k in kinds(p, "filter") {
        s.edit.set_flag(&format!("midi.filter.{k}"), true);
    }
    for k in kinds(p, "allow") {
        s.edit.set_flag(&format!("midi.filter.{k}"), false);
    }
    let filtered: Vec<&str> = MIDI_FILTER_KINDS.iter().copied().filter(|k| s.edit.flag(&format!("midi.filter.{k}"))).collect();
    Ok(json!({"filtered": filtered}))
}

fn device_key(name: &str) -> String {
    let n: String = name.chars().filter(|c| !c.is_control() && *c != '=').take(64).collect();
    format!("midi.input.{}", n.trim())
}

fn midi_inputs(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session_mut();
    for (key, on) in [("enable", true), ("disable", false)] {
        for n in p.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).take(64).collect::<Vec<_>>()).unwrap_or_default() {
            if !n.trim().is_empty() {
                s.edit.set_flag(&device_key(n), on);
            }
        }
    }
    let enabled: Vec<&str> = s.edit.flags.iter().filter_map(|f| f.strip_prefix("midi.input.")).collect();
    Ok(json!({"enabled": enabled}))
}

fn import_session_data(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "file.import_session_data";
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad(id, "`path` required"))?.to_string();
    let text_v = crate::io::read_text(&path)?;
    let mut other = Session::from_json(&text_v).map_err(|err| EngineError::Io(format!("{path}: {err}")))?;
    crate::cmd::mix::restore_plugin_param_ids(&mut other);
    let names: Option<Vec<String>> =
        p.get("tracks").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect());
    let picked: Vec<soundcraft_model::Track> = match &names {
        Some(ns) => {
            let mut v = Vec::new();
            for n in ns {
                v.push(other.track_by_name(n).cloned().ok_or_else(|| bad(id, format!("`{path}` has no track named `{n}`")))?);
            }
            v
        }
        None => other.tracks.iter().filter(|t| t.kind != TrackKind::Master).cloned().collect(),
    };
    if picked.is_empty() {
        return Err(bad(id, "nothing to import"));
    }
    let mut needed: Vec<SourceId> = picked
        .iter()
        .flat_map(|t| t.playlists.iter().flat_map(|pl| pl.clips.iter().flat_map(|c| c.source().into_iter().chain(c.clean_alt().map(|k| k.source)))))
        .collect();
    // A Sampler's sample is session audio too.
    needed.extend(picked.iter().filter_map(|t| t.instrument.as_ref()).flat_map(|i| i.sources()));
    needed.sort_by_key(|x| x.0);
    needed.dedup();
    let sr = e.session().sample_rate.hz();
    let s = e.session_mut();
    let mut files = crate::io::DecodedFiles::new();
    let mut map: std::collections::BTreeMap<u64, SourceId> = Default::default();
    let mut missing = Vec::new();
    for src in needed.iter().filter_map(|sid| other.source(*sid)) {
        let mut ns = src.clone();
        ns.id = SourceId(s.alloc());
        match crate::io::stored_path(&src.path, Path::new(&path)) {
            Ok(abs) => {
                ns.path = abs.to_string_lossy().into_owned();
                match crate::io::decode_file(&mut files, &abs, sr) {
                    Ok(audio) => {
                        ns.frames = audio.frames() as u64;
                        ns.sample_rate = sr;
                        ns.unsaved = true;
                        s.pool.insert(ns.id, audio);
                    }
                    Err(_) => {
                        ns.unsaved = false;
                        missing.push(ns.path.clone());
                    }
                }
            }
            Err(why) => {
                ns.unsaved = false;
                missing.push(format!("{}: {why}", ns.path));
            }
        }
        map.insert(src.id.0, ns.id);
        s.sources.push(ns);
    }
    let mut out = Vec::new();
    for mut tr in picked {
        tr.id = TrackId(s.alloc());
        tr.name = s.unique_track_name(&tr.name);
        tr.folder = None;
        tr.mixer.vca = None;
        // Its frozen render is not imported (the donor's source id means nothing here): it plays
        // live.
        tr.frozen = false;
        tr.freeze = None;
        for pl in &mut tr.playlists {
            for c in &mut pl.clips {
                c.id = s.new_clip_id();
                if let ClipContent::Audio { source, .. } = &mut c.content
                    && let Some(n) = map.get(&source.0)
                {
                    *source = *n;
                }
                // Clean Vocal's cleaned audio comes along with its original (or is dropped).
                c.clean = c
                    .clean
                    .and_then(|k| Some(soundcraft_model::CleanAlt { source: *map.get(&k.source.0)?, original: *map.get(&k.original.0)?, ..k }));
            }
        }
        // The donor's source ids mean nothing here: remap the sample, or drop it when it didn't come along.
        if let Some(ins) = tr.instrument.as_mut() {
            ins.sample = ins.sample.and_then(|sid| map.get(&sid.0).copied());
            ins.samples = std::mem::take(&mut ins.samples).into_iter().filter_map(|(k, sid)| Some((k, *map.get(&sid.0)?))).collect();
        }
        // Busses are matched by name (created when missing).
        let remap = |s: &mut Session, r: &Route| -> Route {
            match r {
                Route::Bus(b) => other.bus(*b).map_or(Route::None, |ob| Route::Bus(s.add_bus(&ob.name, ob.format))),
                x => x.clone(),
            }
        };
        tr.mixer.input = remap(s, &tr.mixer.input);
        tr.mixer.output = remap(s, &tr.mixer.output);
        for slot in tr.mixer.sends.iter_mut().flatten() {
            slot.target = remap(s, &slot.target);
        }
        let pos = s.tracks.iter().position(|t| t.kind == TrackKind::Master).unwrap_or(s.tracks.len());
        out.push(tr.id);
        s.tracks.insert(pos, tr);
    }
    // Their modules' assets (an Amp's IR): made ready here, or reported missing.
    missing.extend(crate::cmd::module_asset::import_track_assets(s, &out, Path::new(&path)));
    s.edit.selected_tracks = out.clone();
    Ok(json!({"tracks": out, "sources": map.len(), "missing": missing}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::InputMonitor;

    #[test]
    fn option_flags_toggle() {
        let mut e = Engine::default();
        assert_eq!(e.execute("options.midi_thru", &json!({})).unwrap()["value"], true);
        assert!(e.session().edit.flag("options.midi_thru"));
        e.execute("options.midi_thru", &json!({"value": false})).unwrap();
        assert!(!e.session().edit.flag("options.midi_thru"));
    }

    #[test]
    fn io_bus_lifecycle() {
        let mut e = crate::demo::demo_engine();
        e.execute("setup.io", &json!({"action": "create_bus", "name": "Drums"})).unwrap();
        e.execute("setup.io", &json!({"action": "rename_bus", "name": "Verb", "new_name": "Hall"})).unwrap();
        assert!(e.session().bus_by_name("Hall").is_some());
        e.execute("setup.io", &json!({"action": "delete_bus", "name": "Hall"})).unwrap();
        assert!(
            e.session().tracks.iter().all(|t| t
                .mixer
                .sends
                .iter()
                .flatten()
                .all(|s| matches!(s.target, Route::None | Route::Main | Route::Hardware(_))))
        );
        assert!(e.session().tracks.iter().all(|t| {
            !matches!(t.mixer.input, Route::Bus(_))
                || e.session()
                    .bus(match t.mixer.input {
                        Route::Bus(b) => b,
                        _ => BusId(0),
                    })
                    .is_some()
        }));
        e.execute("setup.io", &json!({"action": "create_output", "name": "Out 3-4", "first_channel": 2})).unwrap();
        assert_eq!(e.session().outputs.len(), 2);
        assert!(e.execute("setup.io", &json!({"action": "nope", "name": "x"})).is_err());
        assert!(e.can_undo());
    }

    fn io(e: &mut Engine, p: Value) -> Result<Value> {
        e.execute("setup.io", &p)
    }

    fn input_of(e: &Engine, track: &str) -> Route {
        e.session().track_by_name(track).unwrap().mixer.input.clone()
    }

    fn new_track(e: &mut Engine, name: &str, format: &str) -> u64 {
        e.execute("track.new", &json!({"name": name, "format": format})).unwrap()["tracks"][0].as_u64().unwrap()
    }

    #[test]
    fn io_inputs_are_created_renamed_remapped_and_deleted() {
        let mut e = Engine::default();
        let vox = new_track(&mut e, "Vox", "mono");
        let r = io(&mut e, json!({"action": "create_input", "name": "Vox Mic", "format": "mono", "first_channel": 4})).unwrap();
        assert_eq!(r["inputs"][0]["name"], "Vox Mic");
        assert_eq!(r["inputs"][0]["first_channel"], 4);
        assert!(r.get("busses").is_some() && r.get("outputs").is_some());
        assert!(io(&mut e, json!({"action": "create_input", "name": "vox mic"})).is_err(), "names are unique");
        let err = io(&mut e, json!({"action": "create_input", "name": "Room", "format": "5.1"})).unwrap_err().to_string();
        assert!(err.contains("inputs are mono or stereo"), "{err}");
        e.execute("track.input", &json!({"tracks": [vox], "input": "Vox Mic"})).unwrap();

        io(&mut e, json!({"action": "rename_input", "name": "Vox Mic", "new_name": "Lead"})).unwrap();
        assert_eq!(e.session().inputs[0].name, "Lead");
        assert_eq!(input_of(&e, "Vox"), Route::Hardware("Lead".into()));
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().inputs[0].name, "Vox Mic");
        assert_eq!(input_of(&e, "Vox"), Route::Hardware("Vox Mic".into()));

        io(&mut e, json!({"action": "set_input_channel", "name": "vox mic", "first_channel": 99_999})).unwrap();
        assert_eq!(e.session().inputs[0].first_channel, 1023);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().inputs[0].first_channel, 4);

        io(&mut e, json!({"action": "set_output_channel", "name": "Out 1-2", "first_channel": 2})).unwrap();
        assert_eq!(e.session().outputs[0].first_channel, 2);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().outputs[0].first_channel, 0);
        assert!(io(&mut e, json!({"action": "set_output_channel", "name": "Nope", "first_channel": 2})).is_err());

        io(&mut e, json!({"action": "delete_input", "name": "Vox Mic"})).unwrap();
        assert!(e.session().inputs.is_empty());
        assert_eq!(input_of(&e, "Vox"), Route::None);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().inputs.len(), 1);
        assert_eq!(input_of(&e, "Vox"), Route::Hardware("Vox Mic".into()));
        assert!(io(&mut e, json!({"action": "delete_input", "name": "Nope"})).is_err());

        let listed = io(&mut e, json!({})).unwrap();
        assert_eq!(listed["inputs"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn auto_inputs_adds_missing_names_only() {
        let mut e = Engine::default();
        io(&mut e, json!({"action": "create_input", "name": "in 2", "format": "stereo", "first_channel": 7})).unwrap();
        let r = io(&mut e, json!({"action": "auto_inputs", "channels": 4})).unwrap();
        let rows: Vec<(String, u64, String)> = r["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| (i["name"].as_str().unwrap().to_string(), i["first_channel"].as_u64().unwrap(), i["format"].to_string()))
            .collect();
        let names: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(names, ["in 2", "In 1", "In 3", "In 4"]);
        assert_eq!(rows[0].1, 7, "the existing input is kept as it was");
        assert_eq!((rows[2].1, rows[3].1), (2, 3));
        assert_eq!(e.session().inputs[1].format, ChannelFormat::Mono);
        io(&mut e, json!({"action": "auto_inputs", "channels": 1000})).unwrap();
        assert_eq!(e.session().inputs.len(), 64);
        assert!(io(&mut e, json!({"action": "auto_inputs"})).is_err());
    }

    #[test]
    fn reserved_route_names_are_refused_for_paths() {
        let mut e = Engine::default();
        io(&mut e, json!({"action": "create_output", "name": "Mon 5-6", "first_channel": 4})).unwrap();
        io(&mut e, json!({"action": "create_input", "name": "Vox", "first_channel": 0})).unwrap();
        // These names always mean "no route" or the main: a path called that could never be picked.
        for name in ["Main", "none", "OUT 1-2", "No Input", "no output", "Master"] {
            let err = io(&mut e, json!({"action": "create_input", "name": name})).unwrap_err().to_string();
            assert!(err.contains("reserved"), "{name}: {err}");
            assert!(io(&mut e, json!({"action": "create_output", "name": name})).is_err(), "{name}");
            assert!(io(&mut e, json!({"action": "rename_input", "name": "Vox", "new_name": name})).is_err(), "{name}");
            assert!(io(&mut e, json!({"action": "rename_output", "name": "Mon 5-6", "new_name": name})).is_err(), "{name}");
        }
        assert_eq!(e.session().inputs[0].name, "Vox");
        assert_eq!(e.session().outputs[1].name, "Mon 5-6");
        // The main may carry a main name (it is the main), never "none".
        io(&mut e, json!({"action": "rename_output", "name": "Out 1-2", "new_name": "Main"})).unwrap();
        io(&mut e, json!({"action": "rename_output", "name": "main", "new_name": "Out 1-2"})).unwrap();
        assert!(io(&mut e, json!({"action": "rename_output", "name": "Out 1-2", "new_name": "None"})).is_err());
        // Output names stay unique, case-insensitively, as input names do.
        assert!(io(&mut e, json!({"action": "rename_output", "name": "Mon 5-6", "new_name": "out 1-2 "})).is_err());
        io(&mut e, json!({"action": "create_output", "name": "Phones", "first_channel": 6})).unwrap();
        assert!(io(&mut e, json!({"action": "rename_output", "name": "Phones", "new_name": "MON 5-6"})).is_err());
        assert!(io(&mut e, json!({"action": "delete_output", "name": "out 1-2"})).is_err(), "the main stays");
    }

    #[test]
    fn busses_refuse_reserved_names() {
        let mut e = Engine::default();
        for name in ["Main", "none", "OUT 1-2", "No Output", "master"] {
            let err = io(&mut e, json!({"action": "create_bus", "name": name})).unwrap_err().to_string();
            assert!(err.contains("reserved"), "{name}: {err}");
        }
        io(&mut e, json!({"action": "create_bus", "name": "Drums"})).unwrap();
        assert!(io(&mut e, json!({"action": "rename_bus", "name": "Drums", "new_name": "Main"})).is_err());
        assert!(e.session().bus_by_name("Drums").is_some());
    }

    #[test]
    fn set_output_format_says_which_output_was_wrong() {
        let mut e = Engine::default();
        let main = io(&mut e, json!({"action": "set_output_format", "name": "Out 1-2", "format": "mono"})).unwrap_err().to_string();
        assert!(main.contains("setup.main_format") && !main.contains("no output"), "{main}");
        let none = io(&mut e, json!({"action": "set_output_format", "name": "Nope", "format": "mono"})).unwrap_err().to_string();
        assert!(none.contains("no output `Nope`") && !none.contains("main_format"), "{none}");
    }

    #[test]
    fn renaming_an_output_carries_routes_case_insensitively() {
        let mut e = Engine::default();
        let t = new_track(&mut e, "Vox", "mono");
        io(&mut e, json!({"action": "create_output", "name": "Mon 5-6", "first_channel": 4})).unwrap();
        e.execute("track.output", &json!({"tracks": [t], "output": "Mon 5-6"})).unwrap();
        // A route stored with other capitals (a hand-edited or older session) follows too.
        e.session_mut().tracks.iter_mut().for_each(|tr| {
            if tr.id.0 == t {
                tr.mixer.output = Route::Hardware("MON 5-6".into());
            }
        });
        io(&mut e, json!({"action": "rename_output", "name": "mon 5-6", "new_name": "Cue"})).unwrap();
        assert_eq!(e.session().track_by_name("Vox").unwrap().mixer.output, Route::Hardware("Cue".into()));
    }

    #[test]
    fn create_input_and_auto_inputs_undo_as_one_step_each() {
        let mut e = Engine::default();
        io(&mut e, json!({"action": "create_input", "name": "Vox", "format": "stereo", "first_channel": 2})).unwrap();
        io(&mut e, json!({"action": "auto_inputs", "channels": 8})).unwrap();
        assert_eq!(e.session().inputs.len(), 9);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().inputs.len(), 1, "auto_inputs undoes as one step");
        assert_eq!(e.session().inputs[0].name, "Vox");
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().inputs.is_empty(), "create_input undoes");
        e.execute("edit.redo", &json!({})).unwrap();
        e.execute("edit.redo", &json!({})).unwrap();
        assert_eq!(e.session().inputs.len(), 9);
    }

    #[test]
    fn input_and_output_formats_change_as_one_step() {
        let mut e = Engine::default();
        io(&mut e, json!({"action": "create_input", "name": "Keys", "first_channel": 2})).unwrap();
        io(&mut e, json!({"action": "create_output", "name": "Mon", "first_channel": 4})).unwrap();
        io(&mut e, json!({"action": "set_input_format", "name": "keys", "format": "stereo"})).unwrap();
        assert_eq!(e.session().inputs[0].format, ChannelFormat::Stereo);
        assert!(io(&mut e, json!({"action": "set_input_format", "name": "Keys", "format": "5.1"})).is_err());
        assert!(io(&mut e, json!({"action": "set_input_format", "name": "Keys"})).is_err());
        assert!(io(&mut e, json!({"action": "set_input_format", "name": "Nope", "format": "mono"})).is_err());
        io(&mut e, json!({"action": "set_output_format", "name": "Mon", "format": "mono"})).unwrap();
        assert_eq!(e.session().outputs[1].format, ChannelFormat::Mono);
        assert!(io(&mut e, json!({"action": "set_output_format", "name": "Out 1-2", "format": "mono"})).is_err(), "the main's is setup.main_format");
        assert!(io(&mut e, json!({"action": "set_output_format", "name": "Mon"})).is_err());
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().outputs[1].format, ChannelFormat::Stereo);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().inputs[0].format, ChannelFormat::Mono);
    }

    #[test]
    fn named_paths_never_become_busses() {
        let mut e = Engine::default();
        let t = new_track(&mut e, "Vox", "mono");
        io(&mut e, json!({"action": "create_input", "name": "Vox Mic", "format": "mono", "first_channel": 0})).unwrap();
        io(&mut e, json!({"action": "create_output", "name": "Mon 5-6", "first_channel": 4})).unwrap();
        let busses = e.session().busses.len();
        e.execute("track.input", &json!({"tracks": [t], "input": "vox mic"})).unwrap();
        assert_eq!(input_of(&e, "Vox"), Route::Hardware("Vox Mic".into()));
        e.execute("track.output", &json!({"tracks": [t], "output": "MON 5-6"})).unwrap();
        assert_eq!(e.session().track_by_name("Vox").unwrap().mixer.output, Route::Hardware("Mon 5-6".into()));
        io(&mut e, json!({"action": "rename_output", "name": "Out 1-2", "new_name": "Mains"})).unwrap();
        e.execute("track.output", &json!({"tracks": [t], "output": "Mains"})).unwrap();
        assert_eq!(e.session().track_by_name("Vox").unwrap().mixer.output, Route::Main);
        assert_eq!(e.session().busses.len(), busses);
        // An unknown name still makes a bus, as before.
        e.execute("track.output", &json!({"tracks": [t], "output": "Drums"})).unwrap();
        assert_eq!(e.session().busses.len(), busses + 1);
    }

    #[test]
    fn input_monitor_takes_three_states_and_cycles() {
        let mut e = Engine::default();
        let t = new_track(&mut e, "Vox", "mono");
        let u = new_track(&mut e, "Gtr", "mono");
        let state = |e: &Engine, n: &str| e.session().track_by_name(n).unwrap().mixer.input_monitor;
        let set = |e: &mut Engine, v: Value| e.execute("mix.input_monitor", &json!({"tracks": [t, u], "value": v})).unwrap()["value"].clone();
        assert_eq!(set(&mut e, json!(true)), "on");
        assert_eq!(state(&e, "Vox"), InputMonitor::On);
        assert_eq!(set(&mut e, json!(false)), "auto");
        assert_eq!(state(&e, "Gtr"), InputMonitor::Auto);
        assert_eq!(set(&mut e, json!("off")), "off");
        assert_eq!(state(&e, "Vox"), InputMonitor::Off);
        assert!(e.execute("mix.input_monitor", &json!({"tracks": [t], "value": "loud"})).is_err());
        assert!(e.execute("mix.input_monitor", &json!({"tracks": [t], "value": 3})).is_err());
        e.execute("mix.input_monitor", &json!({"tracks": [t], "value": "auto"})).unwrap();
        let cycle = |e: &mut Engine| e.execute("mix.input_monitor", &json!({"tracks": [t, u]})).unwrap()["value"].clone();
        assert_eq!(cycle(&mut e), "on");
        assert_eq!((state(&e, "Vox"), state(&e, "Gtr")), (InputMonitor::On, InputMonitor::On));
        assert_eq!(cycle(&mut e), "off");
        assert_eq!(cycle(&mut e), "auto");
        assert_eq!(state(&e, "Gtr"), InputMonitor::Auto);
    }

    #[test]
    fn current_timecode_sets_start() {
        let mut e = Engine::default();
        e.execute("edit.select", &json!({"start": 48_000})).unwrap();
        e.execute("setup.current_timecode", &json!({"timecode": "01:00:00:00"})).unwrap();
        let s = e.session();
        let shown = s.format_position(48_000, soundcraft_time::TimeFormat::Timecode);
        assert_eq!(shown, "01:00:00:00");
    }

    #[test]
    fn click_countoff_settings() {
        let mut e = Engine::default();
        let r = e.execute("setup.click_countoff", &json!({"volume_db": -12.0, "countoff_bars": 4, "countoff": true})).unwrap();
        assert_eq!(r["volume_db"], -12.0);
        assert_eq!(e.session().edit.countoff_bars, 4);
        assert!(e.session().edit.countoff);
    }

    #[test]
    fn shortcuts_listed() {
        let mut e = Engine::default();
        let r = e.execute("setup.keyboard_shortcuts", &json!({})).unwrap();
        assert!(r["shortcuts"].as_array().unwrap().iter().any(|x| x["id"] == "edit.undo"));
    }

    #[test]
    fn engine_and_midi_settings() {
        let mut e = Engine::default();
        let r = e.execute("setup.playback_engine", &json!({"buffer_size": 300, "device": "Interface"})).unwrap();
        assert_eq!(r["buffer_size"], 512.0);
        assert_eq!(r["device"], "Interface");
        let r = e.execute("setup.midi_input_filter", &json!({"filter": ["sysex", "bogus"]})).unwrap();
        assert_eq!(r["filtered"], json!(["sysex"]));
        let r = e.execute("setup.midi_input_devices", &json!({"enable": ["Keys"]})).unwrap();
        assert_eq!(r["enabled"], json!(["Keys"]));
        let r = e.execute("setup.video_sync_offset", &json!({"frames": 2})).unwrap();
        assert_eq!(r["samples"], 3200.0);
    }

    #[test]
    fn import_session_data_brings_tracks_and_audio() {
        let dir = std::env::temp_dir().join(format!("sc-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("donor.scraft").to_string_lossy().into_owned();
        let mut donor = crate::demo::demo_engine();
        crate::io::save_session(&mut donor, &path).unwrap();
        let mut e = Engine::default();
        let r = e.execute("file.import_session_data", &json!({"path": path, "tracks": ["Kick", "Keys"]})).unwrap();
        assert_eq!(r["tracks"].as_array().unwrap().len(), 2);
        assert_eq!(r["missing"], json!([]));
        let kick = e.session().track_by_name("Kick").unwrap();
        let src = kick.clips()[0].source().unwrap();
        assert!(e.session().pool.get(src).is_some());
        assert_eq!(e.session().track_by_name("Keys").unwrap().clips().len(), 1);
        assert!(e.execute("file.import_session_data", &json!({"path": path, "tracks": ["Nope"]})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_session_data_brings_a_samplers_sample() {
        let dir = std::env::temp_dir().join(format!("sc-import-sampler-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("hit.wav");
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 4_800]] };
        std::fs::write(&wav, soundcraft_audio_io::encode(&buf, &soundcraft_audio_io::EncodeOptions::default()).unwrap()).unwrap();
        let mut donor = Engine::default();
        donor.execute("track.new", &json!({"count": 1, "name": "Pads", "kind": "instrument"})).unwrap();
        donor.execute("mix.sampler_load", &json!({"track": "Pads", "path": wav.to_string_lossy()})).unwrap();
        let path = dir.join("donor.scraft").to_string_lossy().into_owned();
        crate::io::save_session(&mut donor, &path).unwrap();
        let mut e = crate::demo::demo_engine();
        e.execute("file.import_session_data", &json!({"path": path, "tracks": ["Pads"]})).unwrap();
        let sample = e.session().track_by_name("Pads").and_then(|t| t.instrument.as_ref()).and_then(|i| i.sample).unwrap();
        let src = e.session().source(sample).unwrap();
        assert_eq!(src.name, "hit");
        assert!(e.session().pool.get(sample).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
