//! `track.new_band`: the band set up in one step. A new instrument track for every musician that
//! is switched on and has no track yet, each playing its musician, on an instrument that sounds:
//! an installed plug-in the Library has a target for (MODO DRUM for the drummer, Pianoteq for the
//! piano, …) when the plug-in scan found one, else a built-in instrument with a fitting preset
//! (Juke for drums, a bass, piano, organ, pad or brass sound on the synths). When the song lanes
//! are empty a factory song is laid too, so Play is heard at once. One undo step.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_band::library::{INSTRUMENT_NAMES, LibraryData, auto_target};
use soundcraft_dsp::plugins::fs as presets;
use soundcraft_model::{ChannelFormat, Insert, Route, TrackKind};

/// The song laid when the lanes are empty and no `song` is named.
pub const DEFAULT_SONG: &str = "Pop Verse Chorus Bridge";

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "track.new_band",
        "New Band",
        [],
        None,
        "{song?: factory song name | false (default: Pop Verse Chorus Bridge when the lanes are empty, else none), installed?: [{id, name}] (the installed instrument plug-ins to choose from; default: the plug-in scan's)} → {tracks, song, musicians: [{slot, track, instrument, plugin, preset, installed}]}: an instrument track for each musician that is on and has no track, playing that musician on an installed plug-in its target names, else a built-in instrument with a fitting preset; shows the Sections and Chords rulers",
        always,
        new_band
    )]
}

const C: &str = "track.new_band";

/// The built-in instrument (and its factory preset) a musician playing instrument `i` (an index
/// into `INSTRUMENT_NAMES`) falls back to when no installed plug-in fits.
pub fn fallback(i: u8) -> (&'static str, Option<&'static str>) {
    match i {
        0 => ("fm_synth", Some("Tine Piano")),
        1 => ("wavetable_synth", Some("Glass Keys")),
        2 => ("subtractive_synth", Some("Round Bass")),
        3 => ("fm_synth", Some("Drawn Organ")),
        4 => ("fm_synth", Some("Marimba")),
        5 | 11 => (super::juke::JUKE, None),
        7 => ("subtractive_synth", Some("Warm Pad")),
        8 => ("subtractive_synth", Some("Square Lead")),
        9 => ("fm_synth", Some("Brass Section")),
        10 => ("wavetable_synth", Some("Step Pluck")),
        _ => ("subtractive_synth", Some("Pluck")),
    }
}

/// An installed instrument plug-in: its id (`clap:…`, `vst3:…`, `au:…`) and name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub id: String,
    pub name: String,
}

/// The installed instrument plug-ins the last scans found (CLAP, VST3, Audio Units).
fn scanned() -> Vec<Installed> {
    let mut out = Vec::new();
    for d in soundcraft_clap_host::cached().into_iter().flatten().filter(|d| d.is_instrument) {
        out.push(Installed { id: d.id, name: d.name });
    }
    for d in soundcraft_vst3_host::cached().into_iter().flatten().filter(|d| d.is_instrument) {
        out.push(Installed { id: d.id, name: d.name });
    }
    for d in soundcraft_au_host::scan().into_iter().filter(|d| d.is_instrument) {
        out.push(Installed { id: d.id, name: d.name });
    }
    out
}

/// The first installed plug-in whose name the Library has a target for (not Generic) that plays
/// instrument `i`: what Auto will then pick for the musician.
pub fn installed_for<'a>(lib: &LibraryData, i: u8, installed: &'a [Installed]) -> Option<&'a Installed> {
    installed.iter().find(|p| {
        let slot = auto_target(lib, &p.name);
        slot != 0 && lib.target(slot as i32).is_some_and(|t| t.instruments.get(usize::from(i)).copied().unwrap_or(false))
    })
}

/// A built-in instrument loaded with one of its factory presets (its values and patch), as the
/// preset menu loads it.
fn builtin(plugin: &str, preset: Option<&str>) -> Result<Insert> {
    if plugin == super::juke::JUKE {
        return Ok(super::juke::juke_insert());
    }
    let info = soundcraft_dsp::plugin_info(plugin).ok_or_else(|| bad(C, format!("unknown plugin `{plugin}`")))?;
    let mut ins = Insert::new(info.id);
    for pi in info.params {
        ins.params.insert(pi.id.to_string(), pi.default);
    }
    let Some(name) = preset else { return Ok(ins) };
    let json = presets::factory_presets(plugin)
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, j)| *j)
        .ok_or_else(|| bad(C, format!("`{plugin}` has no preset `{name}`")))?;
    let p = presets::FsPreset::parse(json, info).ok_or_else(|| bad(C, format!("preset `{name}` can't be read")))?;
    for (k, v) in p.values {
        ins.params.insert(k, v);
    }
    let v: Value = serde_json::from_str(json).map_err(|err| bad(C, format!("preset `{name}`: {err}")))?;
    if let Some(patch) = v.get("modulation").filter(|m| !m.is_null()) {
        ins.modulation = super::synth_mod::parse_patch(C, info.id, patch)?;
    }
    ins.preset = name.to_string();
    super::module_ceiling::prepare_insert(&mut ins);
    Ok(ins)
}

fn installed_param(p: &Value) -> Result<Option<Vec<Installed>>> {
    let Some(v) = p.get("installed") else { return Ok(None) };
    let list = v.as_array().ok_or_else(|| bad(C, "`installed` is a list of {id, name}"))?;
    list.iter()
        .map(|x| {
            let s = |k: &str| x.get(k).and_then(Value::as_str).map(str::to_string);
            match (s("id"), s("name")) {
                (Some(id), Some(name)) => Ok(Installed { id, name }),
                _ => Err(bad(C, "each `installed` entry needs `id` and `name`")),
            }
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

fn new_band(e: &mut Engine, p: &Value) -> Result<Value> {
    let installed = match installed_param(p)? {
        Some(l) => l,
        None => scanned(),
    };
    let lib = e.band_library();
    let session = e.session();
    let slots: Vec<(usize, u8)> = session
        .band
        .slots
        .iter()
        .enumerate()
        .filter(|(k, m)| m.enabled && !session.tracks.iter().any(|t| t.mixer.input == Route::Musician(*k as u8)))
        .map(|(k, m)| (k, m.instrument))
        .collect();
    if slots.is_empty() {
        let on = session.band.slots.iter().any(|m| m.enabled);
        return Err(bad(C, if on { "every musician who is on already has a track" } else { "every musician is switched off" }));
    }
    let empty = session.band.sections.is_empty() && session.band.chords.is_empty();
    let song = match p.get("song") {
        None => empty.then(|| DEFAULT_SONG.to_string()),
        Some(Value::Bool(false)) | Some(Value::Null) => None,
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(v) => return Err(bad(C, format!("`song` is a factory song's name or false (got {v})"))),
    };
    // Every instrument before touching the session, so a refusal changes nothing.
    let mut plan = Vec::new();
    for (k, i) in slots {
        let (ins, installed_id) = match installed_for(&lib, i, &installed) {
            Some(pl) => (Insert::new(&pl.id), true),
            None => {
                let (plugin, preset) = fallback(i);
                (builtin(plugin, preset)?, false)
            }
        };
        plan.push((k, i, ins, installed_id));
    }
    let laid = match &song {
        Some(name) => Some(super::band::song_factory(e, &json!({"name": name}))?),
        None => None,
    };
    let s = e.session_mut();
    let mut tracks = Vec::new();
    let mut musicians = Vec::new();
    for (k, i, ins, installed_id) in plan {
        let name = INSTRUMENT_NAMES.get(usize::from(i)).copied().unwrap_or("Band");
        let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some(name));
        let (plugin, preset) = (ins.plugin.clone(), ins.preset.clone());
        if let Some(tr) = s.track_mut(t) {
            tr.instrument = Some(ins);
            tr.mixer.input = Route::Musician(k as u8);
        }
        tracks.push(t);
        musicians.push(json!({"slot": k + 1, "track": t, "instrument": name, "plugin": plugin, "preset": preset, "installed": installed_id}));
    }
    s.edit.selected_tracks = tracks.clone();
    super::band::show_lane_rulers(e);
    Ok(json!({"tracks": tracks, "song": laid.and_then(|v| v.get("song").cloned()), "musicians": musicians}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_band::library::embedded_library;

    #[test]
    fn every_fallback_loads() {
        for i in 0..INSTRUMENT_NAMES.len() as u8 {
            let (plugin, preset) = fallback(i);
            let ins = builtin(plugin, preset).unwrap_or_else(|err| panic!("{i}: {err}"));
            assert!(soundcraft_dsp::plugin_info(&ins.plugin).is_some_and(|p| p.is_instrument), "{i}");
        }
    }

    #[test]
    fn an_installed_plugin_is_picked_by_its_target() {
        let lib = embedded_library();
        let list = |names: &[&str]| names.iter().map(|n| Installed { id: format!("vst3:{n}"), name: n.to_string() }).collect::<Vec<_>>();
        let l = list(&["Serum", "MODO DRUM", "MODO BASS 2", "Pianoteq 8 STAGE"]);
        assert_eq!(installed_for(&lib, 5, &l).map(|p| p.name.as_str()), Some("MODO DRUM"));
        assert_eq!(installed_for(&lib, 2, &l).map(|p| p.name.as_str()), Some("MODO BASS 2"));
        assert_eq!(installed_for(&lib, 0, &l).map(|p| p.name.as_str()), Some("Pianoteq 8 STAGE"));
        // A plug-in with no target (Generic) is never chosen; nor one whose target plays another
        // instrument.
        assert_eq!(installed_for(&lib, 9, &l), None);
        assert_eq!(installed_for(&lib, 5, &list(&["Serum"])), None);
    }
}
