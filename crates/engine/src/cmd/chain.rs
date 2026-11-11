//! Plug-in chain presets (`track.save_chain`, `track.load_chain`) and what lets a preset be used
//! in another project: the audio its modules play travels with it.
//!
//! A chain preset is the track's inserts in order, each with its parameters and (a hosted
//! plug-in's) state: `{"format": "freedawm-plugin-chain", "version": 1, "name", "width",
//! "inserts": [...], "samples": {...}}`. Loading one replaces the track's inserts, or adds to them
//! with `append`.
//!
//! A module names the audio it plays (the Sampler's sample, Juke's pad samples) by session source
//! id, which means nothing in another session. So a preset saved here writes each such source as
//! a WAV in `<preset name>_samples/` beside its file and lists them under `samples`
//! (`{"<source id>": {"file": "<preset name>_samples/s<id>.wav", "name"}}`); loading reads them
//! into the session as new sources and points the modules at those. Track presets
//! (`track.save_preset`) do the same.

use super::*;
use crate::cmd;
use serde_json::{Map, json};
use soundcraft_model::{Insert, MAX_MODULES, SourceId};
use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path};

const CHAIN_FORMAT: &str = "freedawm-plugin-chain";
/// Most samples one preset brings in (Juke's pads, layers and round-robins fit many times over).
const MAX_PRESET_SAMPLES: usize = 2048;
/// Largest sample read (frames × channels): ten minutes of 192 kHz stereo, as the Sampler's.
const MAX_SAMPLE_VALUES: usize = 192_000 * 600 * 2;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "track.save_chain", "Save Plug-in Chain...", ["Track"], None, "{path, track?} — writes the track's inserts (parameters, hosted plug-in state) as JSON, and the audio they play beside it", has_selection, save_chain),
        cmd!(
            "track.load_chain",
            "Load Plug-in Chain",
            [],
            None,
            "{path, tracks?, append?: bool} — puts a saved plug-in chain on the tracks in place of their inserts, or after them with `append` → {tracks, modules}",
            has_selection,
            load_chain
        ),
    ]
}

/// The session sources `modules` play (a kit's bus effects included), each once.
fn source_ids(modules: &[&Insert]) -> BTreeSet<SourceId> {
    let mut out = BTreeSet::new();
    for m in modules {
        out.extend(m.sample);
        out.extend(m.samples.values().copied());
        for fx in &m.kit_fx {
            out.extend(fx.insert.sample);
            out.extend(fx.insert.samples.values().copied());
        }
    }
    out
}

/// Writes the audio `modules` play beside the preset at `path`, in `<stem>_samples/`, and returns
/// the preset's `samples` entry. Audio that is not loaded is left out (the module then loads
/// without it). The web build, which saves by download, writes none.
pub(super) fn write_samples(e: &Engine, path: &str, modules: &[&Insert]) -> Result<Value> {
    let mut out = Map::new();
    let ids = source_ids(modules);
    if ids.is_empty() || cfg!(target_arch = "wasm32") {
        return Ok(Value::Object(out));
    }
    let file = Path::new(path);
    let stem = file.file_stem().and_then(|x| x.to_str()).unwrap_or("preset");
    let folder = format!("{stem}_samples");
    let dir = file.parent().unwrap_or_else(|| Path::new("")).join(&folder);
    let s = e.session();
    let mut made = false;
    for id in ids {
        let Some(audio) = s.pool.get(id) else { continue };
        let name = s.sources.iter().find(|x| x.id == id).map_or_else(|| "Sample".to_string(), |x| x.name.clone());
        let bytes = soundcraft_audio_io::encode(
            &audio.buffer(),
            &soundcraft_audio_io::EncodeOptions {
                format: soundcraft_audio_io::FileFormat::Wav,
                bit_depth: soundcraft_audio_io::BitDepth::Int24,
                ..Default::default()
            },
        )
        .map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
        if !made {
            crate::io::make_dir(&dir)?;
            made = true;
        }
        let leaf = format!("s{}.wav", id.0);
        crate::io::write_file(&dir.join(&leaf).to_string_lossy(), &bytes)?;
        out.insert(id.0.to_string(), json!({"file": format!("{folder}/{leaf}"), "name": name}));
    }
    Ok(Value::Object(out))
}

/// The preset's `samples` entry checked, before anything is changed: (the id the preset uses,
/// the file, the name). A file path that leaves the preset's folder is refused.
fn listed_samples(path: &str, preset: &Value) -> Result<Vec<(u64, std::path::PathBuf, String)>> {
    let Some(obj) = preset.get("samples").and_then(Value::as_object) else { return Ok(Vec::new()) };
    if obj.len() > MAX_PRESET_SAMPLES {
        return Err(EngineError::Io(format!("{path}: too many samples")));
    }
    let base = Path::new(path).parent().unwrap_or_else(|| Path::new(""));
    let mut out = Vec::new();
    for (key, entry) in obj {
        let (Ok(id), Some(rel)) = (key.parse::<u64>(), entry.get("file").and_then(Value::as_str)) else { continue };
        let rel = Path::new(rel);
        if rel.as_os_str().is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_))) {
            return Err(EngineError::Io(format!("{path}: a sample's file must be inside the preset's folder")));
        }
        let name: String = entry.get("name").and_then(Value::as_str).unwrap_or("Sample").chars().take(120).collect();
        out.push((id, base.join(rel), name));
    }
    Ok(out)
}

/// Reads the preset's samples into the session as new sources: the id the preset uses to the
/// source here, and how many could not be read (missing, not audio, too long).
fn read_samples(e: &mut Engine, listed: &[(u64, std::path::PathBuf, String)]) -> (HashMap<u64, SourceId>, usize) {
    let mut map = HashMap::new();
    let mut missing = 0;
    for (id, file, name) in listed {
        let rate = e.session().sample_rate.hz();
        let ext = file.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
        let read = crate::io::read_file(file)
            .ok()
            .and_then(|bytes| soundcraft_audio_io::decode(&bytes, ext.as_deref()).ok())
            .filter(|(_, buf)| buf.frames() > 0 && buf.num_channels() > 0)
            .and_then(|(info, buf)| Some((info.format, crate::io::at_rate(buf, rate, MAX_SAMPLE_VALUES)?)));
        match read {
            Some((format, buf)) => {
                map.insert(*id, crate::io::add_source(e.session_mut(), name, buf, None, format));
            }
            None => missing += 1,
        }
    }
    (map, missing)
}

/// Points `ins` at this session's sources (`map`: the preset's id to the source here); a sample
/// that did not come along is dropped.
fn remap(ins: &mut Insert, map: &HashMap<u64, SourceId>) {
    ins.sample = ins.sample.and_then(|s| map.get(&s.0).copied());
    ins.samples = std::mem::take(&mut ins.samples).into_iter().filter_map(|(k, s)| Some((k, *map.get(&s.0)?))).collect();
    for fx in &mut ins.kit_fx {
        remap(&mut fx.insert, map);
    }
}

/// What a preset's modules need to play here: its samples read in and the modules pointed at
/// them. A preset saved before samples were written keeps the ids it has (they mean something
/// only in the session it was saved from).
pub(super) struct Samples {
    listed: Vec<(u64, std::path::PathBuf, String)>,
    carried: bool,
}

impl Samples {
    /// Checks the preset's `samples` entry; nothing is read or changed yet.
    pub(super) fn of(path: &str, preset: &Value) -> Result<Samples> {
        Ok(Samples { listed: listed_samples(path, preset)?, carried: preset.get("samples").is_some_and(Value::is_object) })
    }

    /// Reads the samples in and remaps `modules`; returns how many samples could not be read.
    pub(super) fn attach(&self, e: &mut Engine, modules: &mut [&mut Insert]) -> usize {
        if !self.carried {
            return 0;
        }
        let (map, missing) = read_samples(e, &self.listed);
        for m in modules.iter_mut() {
            remap(m, &map);
        }
        missing
    }
}

fn save_chain(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "track.save_chain";
    let path = str_param(p, "path").filter(|x| !x.trim().is_empty()).ok_or_else(|| bad(id, "`path` required"))?.to_string();
    let first = *tracks_required(e, id, p)?.first().ok_or_else(|| bad(id, "no track"))?;
    let tr = e.session().track(first).ok_or_else(|| bad(id, "no track"))?;
    if tr.mixer.inserts.is_empty() {
        return Err(bad(id, format!("{} has no inserts to save", tr.name)));
    }
    let modules: Vec<&Insert> = tr.mixer.inserts.iter().collect();
    let samples = write_samples(e, &path, &modules)?;
    let name = Path::new(&path).file_stem().and_then(|x| x.to_str()).unwrap_or(&tr.name).to_string();
    let preset = json!({
        "format": CHAIN_FORMAT,
        "version": 1,
        "name": name,
        "width": tr.format.label(),
        "inserts": tr.mixer.inserts,
        "samples": samples,
    });
    let text = serde_json::to_string_pretty(&preset).map_err(|err| EngineError::Io(err.to_string()))?;
    crate::io::write_file(&path, text.as_bytes())?;
    Ok(json!({"path": path, "track": first, "modules": modules.len()}))
}

fn load_chain(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "track.load_chain";
    let path = str_param(p, "path").ok_or_else(|| bad(id, "`path` required"))?.to_string();
    let tracks = tracks_required(e, id, p)?;
    let append = p.get("append").and_then(Value::as_bool).unwrap_or(false);
    let text = crate::io::read_text(&path)?;
    let v: Value = serde_json::from_str(&text).map_err(|err| EngineError::Io(format!("{path}: {err}")))?;
    if v.get("format").and_then(Value::as_str) != Some(CHAIN_FORMAT) {
        return Err(EngineError::Io(format!("{path}: not a FreeDAWm plug-in chain")));
    }
    let mut inserts: Vec<Insert> =
        serde_json::from_value(v.get("inserts").cloned().unwrap_or(Value::Null)).map_err(|err| EngineError::Io(format!("{path}: {err}")))?;
    inserts.truncate(MAX_MODULES);
    if inserts.is_empty() {
        return Err(EngineError::Io(format!("{path}: the chain is empty")));
    }
    let samples = Samples::of(&path, &v)?;
    let missing = samples.attach(e, &mut inserts.iter_mut().collect::<Vec<_>>());
    for ins in &mut inserts {
        // A key input and a link group name things in the session the chain was saved from.
        ins.sidechain = None;
        ins.link_group = None;
    }
    let s = e.session_mut();
    let mut placed = 0;
    for t in &tracks {
        let Some(tr) = s.track_mut(*t) else { continue };
        if !append {
            tr.mixer.inserts.clear();
        }
        for ins in &inserts {
            if tr.mixer.add_module(ins.clone(), None).is_ok() {
                placed += 1;
            }
        }
        tr.mixer.sanitize();
    }
    Ok(json!({"tracks": tracks, "modules": placed, "missing_samples": missing}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::TrackKind;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("fd-chain-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn plugins(e: &Engine, track: &str) -> Vec<String> {
        e.session().track_by_name(track).unwrap().mixer.inserts.iter().map(|i| i.plugin.clone()).collect()
    }

    #[test]
    fn a_chain_goes_to_another_track_and_undoes() {
        let dir = scratch("round");
        let path = dir.join("bass chain.json").to_string_lossy().into_owned();
        let mut e = crate::demo::demo_engine();
        let bass = plugins(&e, "Bass");
        assert!(!bass.is_empty());
        let r = e.execute("track.save_chain", &json!({"path": path, "tracks": ["Bass"]})).unwrap();
        assert_eq!(r["modules"], bass.len());
        let before = e.session().track_by_name("Hats").unwrap().mixer.clone();
        e.execute("track.load_chain", &json!({"path": path, "tracks": ["Hats"]})).unwrap();
        let hats = e.session().track_by_name("Hats").unwrap();
        assert_eq!(plugins(&e, "Hats"), bass);
        assert_eq!(hats.mixer.volume_db, before.volume_db, "only the inserts change");
        assert_eq!(hats.mixer.sends, before.sends);
        let bass_params = e.session().track_by_name("Bass").unwrap().mixer.inserts[0].params.clone();
        assert_eq!(hats.mixer.inserts[0].params, bass_params);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().track_by_name("Hats").unwrap().mixer, before);
        // Appended: after what the track has, each module under its own id.
        e.execute("track.load_chain", &json!({"path": path, "tracks": ["Bass"], "append": true})).unwrap();
        let both = e.session().track_by_name("Bass").unwrap();
        assert_eq!(both.mixer.inserts.len(), bass.len() * 2);
        let ids: BTreeSet<u32> = both.mixer.inserts.iter().map(|i| i.id.0).collect();
        assert_eq!(ids.len(), both.mixer.inserts.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_is_not_a_chain_is_refused() {
        let dir = scratch("junk");
        let mut e = crate::demo::demo_engine();
        let before = e.session().track_by_name("Hats").unwrap().mixer.clone();
        for (name, text) in [
            ("other.json", r#"{"format": "soundcraft-track-preset"}"#),
            ("empty.json", r#"{"format": "freedawm-plugin-chain", "inserts": []}"#),
            ("bad.json", r#"{"format": "freedawm-plugin-chain", "inserts": 5}"#),
            ("escape.json", r#"{"format": "freedawm-plugin-chain", "inserts": [{"plugin": "compressor"}], "samples": {"1": {"file": "../x.wav"}}}"#),
            ("text.json", "not json"),
        ] {
            let p = dir.join(name);
            std::fs::write(&p, text).unwrap();
            assert!(e.execute("track.load_chain", &json!({"path": p.to_string_lossy(), "tracks": ["Hats"]})).is_err(), "{name}");
            assert_eq!(e.session().track_by_name("Hats").unwrap().mixer, before, "{name}");
        }
        // A track with nothing on it has no chain to save.
        e.execute("track.new", &json!({"name": "Bare", "kind": "audio"})).unwrap();
        assert!(e.execute("track.save_chain", &json!({"path": dir.join("bare.json").to_string_lossy(), "tracks": ["Bare"]})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A second of a 440 Hz tone as a mono 48 kHz WAV.
    fn tone_wav(path: &Path) {
        let frames = 48_000;
        let ch: Vec<f32> = (0..frames).map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5).collect();
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![ch] };
        let bytes = soundcraft_audio_io::encode(&buf, &soundcraft_audio_io::EncodeOptions::default()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn a_track_preset_carries_its_sample_to_another_session() {
        let dir = scratch("sample");
        let wav = dir.join("tone.wav");
        tone_wav(&wav);
        let path = dir.join("keys.json").to_string_lossy().into_owned();
        let mut a = Engine::default();
        a.execute("track.new", &json!({"name": "Keys", "kind": "instrument"})).unwrap();
        a.execute("mix.sampler_load", &json!({"track": "Keys", "path": wav.to_string_lossy()})).unwrap();
        a.execute("track.save_preset", &json!({"path": path, "tracks": ["Keys"]})).unwrap();
        std::fs::remove_file(&wav).unwrap();
        assert!(dir.join("keys_samples").is_dir());

        // Another session: other ids, and nothing of the first one's audio.
        let mut b = Engine::default();
        for n in ["One", "Two"] {
            b.execute("track.new", &json!({"name": n, "kind": "audio"})).unwrap();
        }
        b.execute("track.new", &json!({"name": "Pad", "kind": "instrument"})).unwrap();
        let r = b.execute("track.load_preset", &json!({"path": path, "tracks": ["Pad"]})).unwrap();
        assert_eq!(r["missing_samples"], 0);
        let tr = b.session().track_by_name("Pad").unwrap();
        assert_eq!(tr.kind, TrackKind::Instrument);
        let ins = tr.instrument.as_ref().unwrap();
        assert_eq!(ins.plugin, "sampler");
        let src = ins.sample.expect("the sample came along");
        assert_eq!(b.session().pool.get(src).unwrap().frames(), 48_000);
        assert_eq!(b.session().sources.iter().find(|s| s.id == src).unwrap().name, "tone");

        // Without its samples folder the preset still loads, the sampler empty.
        std::fs::remove_dir_all(dir.join("keys_samples")).unwrap();
        let mut c = Engine::default();
        c.execute("track.new", &json!({"name": "Pad", "kind": "instrument"})).unwrap();
        let r = c.execute("track.load_preset", &json!({"path": path, "tracks": ["Pad"]})).unwrap();
        assert_eq!(r["missing_samples"], 1);
        assert_eq!(c.session().track_by_name("Pad").unwrap().instrument.as_ref().unwrap().sample, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_track_preset_finds_its_send_bus_by_name() {
        let dir = scratch("bus");
        let path = dir.join("vox.json").to_string_lossy().into_owned();
        let mut a = Engine::default();
        a.execute("track.new", &json!({"name": "Vox", "kind": "audio"})).unwrap();
        let (vox, bus) = {
            let s = a.session_mut();
            let bus = s.add_bus("Verb", soundcraft_model::ChannelFormat::Stereo);
            let vox = s.track_by_name("Vox").unwrap().id;
            s.track_mut(vox).unwrap().mixer.sends[0] = Some(soundcraft_model::SendSlot::new(soundcraft_model::Route::Bus(bus)));
            (vox, bus)
        };
        a.execute("track.save_preset", &json!({"path": path, "track": vox.0})).unwrap();

        let mut b = Engine::default();
        // Ids here differ: a bus of another name takes the first id.
        b.session_mut().add_bus("Drums", soundcraft_model::ChannelFormat::Stereo);
        b.execute("track.new", &json!({"name": "Lead", "kind": "audio"})).unwrap();
        b.execute("track.load_preset", &json!({"path": path, "tracks": ["Lead"]})).unwrap();
        let s = b.session();
        let verb = s.bus_by_name("Verb").expect("the send's bus was made").id;
        let send = s.track_by_name("Lead").unwrap().mixer.sends[0].clone().unwrap();
        assert_eq!(send.target, soundcraft_model::Route::Bus(verb));
        let _ = bus;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
